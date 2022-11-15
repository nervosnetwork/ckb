mod block_fetcher;
mod block_process;
mod get_blocks_process;
mod get_headers_process;
mod headers_process;
mod in_ibd_process;

pub(crate) use self::block_fetcher::BlockFetcher;
pub(crate) use self::block_process::BlockProcess;
pub(crate) use self::get_blocks_process::GetBlocksProcess;
pub(crate) use self::get_headers_process::GetHeadersProcess;
pub(crate) use self::headers_process::HeadersProcess;
pub(crate) use self::in_ibd_process::InIBDProcess;
use crate::block_status::BlockStatus;
use crate::types::{HeaderView, HeadersSyncController, IBDState, Peers, SyncShared};
use crate::utils::send_message_to;
use crate::{Status, StatusCode};
use crossbeam::queue::ArrayQueue;

use futures::prelude::*;

use ckb_chain::chain::ChainController;
use ckb_channel as channel;
use ckb_constant::sync::{
    BAD_MESSAGE_BAN_TIME, CHAIN_SYNC_TIMEOUT, EVICTION_HEADERS_RESPONSE_TIME,
    INIT_BLOCKS_IN_TRANSIT_PER_PEER, MAX_TIP_AGE,
};
use ckb_error::Error as CKBError;
use ckb_logger::{debug, error, info, trace, warn};
use ckb_metrics::metrics;
use ckb_network::{
    async_trait, bytes::Bytes, tokio, CKBProtocolContext, CKBProtocolHandler, PeerIndex,
    ServiceControl, SupportProtocols,
};
use ckb_types::core::BlockView;
use ckb_types::molecule::Number;
use ckb_types::{
    core::{self, BlockNumber},
    packed::{self, Byte32},
    prelude::*,
};

use faketime::unix_time_as_millis;

use crossbeam::sync::ShardedLock;
use std::{
    collections::HashSet,
    sync::{atomic::Ordering, Arc},
    thread,
    time::{Duration, Instant},
};

pub const SEND_GET_HEADERS_TOKEN: u64 = 0;
pub const IBD_BLOCK_FETCH_TOKEN: u64 = 1;
pub const NOT_IBD_BLOCK_FETCH_TOKEN: u64 = 2;
pub const TIMEOUT_EVICTION_TOKEN: u64 = 3;
pub const NO_PEER_CHECK_TOKEN: u64 = 255;

const SYNC_NOTIFY_INTERVAL: Duration = Duration::from_secs(1);
const IBD_BLOCK_FETCH_INTERVAL: Duration = Duration::from_millis(40);
const NOT_IBD_BLOCK_FETCH_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Copy, Clone)]
enum CanStart {
    Ready,
    MinWorkNotReach,
    AssumeValidNotFound,
}

enum FetchCMD {
    Fetch((Vec<PeerIndex>, IBDState)),
}

struct BlockFetchCMD {
    sync: Synchronizer,
    p2p_control: ServiceControl,
    recv: channel::Receiver<FetchCMD>,
    can_start: CanStart,
    number: BlockNumber,
}

impl BlockFetchCMD {
    fn run(&mut self) {
        while let Ok(cmd) = self.recv.recv() {
            match cmd {
                FetchCMD::Fetch((peers, state)) => match self.can_start() {
                    CanStart::Ready => {
                        for peer in peers {
                            if let Some(fetch) = BlockFetcher::new(&self.sync, peer, state).fetch()
                            {
                                for item in fetch {
                                    BlockFetchCMD::send_getblocks(item, &self.p2p_control, peer);
                                }
                            }
                        }
                    }
                    CanStart::MinWorkNotReach => {
                        let best_known = self.sync.shared.state().shared_best_header_ref();
                        let number = best_known.number();
                        if number != self.number && (number - self.number) % 10000 == 0 {
                            self.number = number;
                            info!(
                                    "best known header number: {}, total difficulty: {:#x}, \
                                 require min header number on 500_000, min total difficulty: {:#x}, \
                                 then start to download block",
                                    number,
                                    best_known.total_difficulty(),
                                    self.sync.shared.state().min_chain_work()
                                );
                        }
                    }
                    CanStart::AssumeValidNotFound => {
                        let state = self.sync.shared.state();
                        let best_known = state.shared_best_header_ref();
                        let number = best_known.number();
                        let assume_valid_target: Byte32 = state
                            .assume_valid_target()
                            .as_ref()
                            .map(Pack::pack)
                            .expect("assume valid target must exist");

                        if number != self.number && (number - self.number) % 10000 == 0 {
                            self.number = number;
                            info!(
                                "best known header number: {}, hash: {:#?}, \
                                 can't find assume valid target temporarily, hash: {:#?} \
                                 please wait",
                                number,
                                best_known.hash(),
                                assume_valid_target
                            );
                        }
                    }
                },
            }
        }
    }

    fn can_start(&mut self) -> CanStart {
        if let CanStart::Ready = self.can_start {
            return self.can_start;
        }

        let state = self.sync.shared.state();

        let min_work_reach = |flag: &mut CanStart| {
            if state.min_chain_work_ready() {
                *flag = CanStart::AssumeValidNotFound;
            }
        };

        let assume_valid_target_find = |flag: &mut CanStart| {
            let mut assume_valid_target = state.assume_valid_target();
            if let Some(ref target) = *assume_valid_target {
                match state.header_map().get(&target.pack()) {
                    Some(header) => {
                        *flag = CanStart::Ready;
                        // Blocks that are no longer in the scope of ibd must be forced to verify
                        if unix_time_as_millis().saturating_sub(header.timestamp()) < MAX_TIP_AGE {
                            assume_valid_target.take();
                        }
                    }
                    None => {
                        // Best known already not in the scope of ibd, it means target is invalid
                        if unix_time_as_millis()
                            .saturating_sub(state.shared_best_header_ref().timestamp())
                            < MAX_TIP_AGE
                        {
                            *flag = CanStart::Ready;
                            assume_valid_target.take();
                        }
                    }
                }
            } else {
                *flag = CanStart::Ready;
            }
        };

        match self.can_start {
            CanStart::Ready => self.can_start,
            CanStart::MinWorkNotReach => {
                min_work_reach(&mut self.can_start);
                if let CanStart::AssumeValidNotFound = self.can_start {
                    assume_valid_target_find(&mut self.can_start);
                }
                self.can_start
            }
            CanStart::AssumeValidNotFound => {
                assume_valid_target_find(&mut self.can_start);
                self.can_start
            }
        }
    }

    fn send_getblocks(v_fetch: Vec<packed::Byte32>, nc: &ServiceControl, peer: PeerIndex) {
        let content = packed::GetBlocks::new_builder()
            .block_hashes(v_fetch.clone().pack())
            .build();
        let message = packed::SyncMessage::new_builder().set(content).build();

        debug!("send_getblocks len={:?} to peer={}", v_fetch.len(), peer);
        if let Err(err) = nc.send_message_to(
            peer,
            SupportProtocols::Sync.protocol_id(),
            message.as_bytes(),
        ) {
            debug!("synchronizer send GetBlocks error: {:?}", err);
        }
    }
}

/// SendBlock msg related info
pub struct SendBlockMsgInfo {
    peer: PeerIndex,
    item_name: String,
    item_bytes_length: u64,
    item_id: Number,
}

type ShardedOption<T> = Arc<ShardedLock<Option<T>>>;

/// Sync protocol handle
pub struct Synchronizer {
    pub(crate) chain: ChainController,
    /// Sync shared state
    pub shared: Arc<SyncShared>,
    fetch_channel: Option<channel::Sender<FetchCMD>>,

    /// Only in IBD mode, downloaded blocks will be pushed to block_queue
    /// The block_queue will be consumed by ProcessBlock thread
    /// If IBD finished, and block_queue will be dropped when the queue is  empty
    block_queue: ShardedOption<ArrayQueue<(BlockView, SendBlockMsgInfo)>>,
    /// Only in IBD mode, if no blocks in queue, the consumer thread will park(),
    /// and be notified by this thread handle
    block_queue_consumer_handle: ShardedOption<thread::JoinHandle<()>>,
    /// The channel Receiver is used for CKBProtocolHandler::poll()
    /// If we got process_new_block's status from this receiver, give the status to post_process()
    block_queue_consume_status_recv:
        Option<futures::channel::mpsc::Receiver<(Status, SendBlockMsgInfo)>>,
}

impl Clone for Synchronizer {
    fn clone(&self) -> Self {
        Synchronizer {
            chain: self.chain.clone(),
            shared: Arc::clone(&self.shared),
            fetch_channel: self.fetch_channel.clone(),
            block_queue: Arc::clone(&self.block_queue),
            block_queue_consumer_handle: Arc::clone(&self.block_queue_consumer_handle),

            // we only need one Receiver for CKBProtocolHandler::poll
            block_queue_consume_status_recv: None,
        }
    }
}

impl Synchronizer {
    /// Init sync protocol handle
    ///
    /// This is a runtime sync protocol shared state, and any relay messages will be processed and forwarded by it
    pub fn new(chain: ChainController, shared: Arc<SyncShared>) -> Synchronizer {
        let (mut status_sender, status_recv) = futures::channel::mpsc::channel(512);

        let mut sync = Synchronizer {
            chain,
            shared,
            fetch_channel: None,
            block_queue: Arc::new(ShardedLock::new(None)),
            block_queue_consumer_handle: Arc::new(ShardedLock::new(None)),

            // only main Synchronizer instance hold status_recv, the clone hold None
            block_queue_consume_status_recv: Some(status_recv),
        };

        let sync_clone = sync.clone();

        // only create block queue and consumer thread in ibd mode
        if sync_clone
            .shared()
            .active_chain()
            .is_initial_block_download()
        {
            let _ = sync_clone
                .block_queue
                .write()
                .expect("Synchronizer wants to acquire write lock on block_queue to fill the block_queue, but it has poisoned")
                .replace(ArrayQueue::new(512));

            let thread_handle = thread::Builder::new()
                .name("ProcessBlock".to_string())
                .spawn(move || loop {
                    if let Some(block_queue) = sync_clone.block_queue.read().expect("Synchronizer wants to acquire read lock on block_queue to consume the block_queue, but it has poisoned").as_ref() {
                        debug!(
                            "block queue's len()/capacity() = {}/{}",
                            block_queue.len(),
                            block_queue.capacity()
                        );

                        while let Some((
                            block,
                            SendBlockMsgInfo {
                                peer,
                                item_name,
                                item_bytes_length,
                                item_id,
                            },
                        )) = block_queue.pop()
                        {
                            debug!(
                                "get block from block_queue, height: {}",
                                block.number() as u64
                            );
                            let hash = block.hash();
                            let mut status = Status::ok();
                            if let Err(err) = sync_clone.process_new_block(block) {
                                if !crate::utils::is_internal_db_error(&err) {
                                    error!("BlockAcceptCMD process_new_block error: {}", err);

                                    status = StatusCode::BlockIsInvalid
                                        .with_context(format!("{}, error: {}", hash, err,));

                                    Self::metrics_block_process(
                                        item_bytes_length,
                                        item_id,
                                        &status,
                                    );

                                    // Only report status when not ok
                                    let _ = status_sender.try_send((
                                        status,
                                        SendBlockMsgInfo {
                                            peer,
                                            item_name,
                                            item_bytes_length,
                                            item_id,
                                        },
                                    ));
                                }
                            } else {
                                Self::metrics_block_process(item_bytes_length, item_id, &status);
                            }
                        }
                    } else {
                        // block_queue was dropped, the thread exit
                        return;
                    }
                    thread::sleep(IBD_BLOCK_FETCH_INTERVAL / 4);
                })
                .expect("block queue and consumer thread can't start");

            let _ = sync
                .block_queue_consumer_handle
                .write()
                .expect("Synchronizer wants to acquire write lock on block_queue_consumer_handle to fill the thread_handle, but it has poisoned")
                .replace(thread_handle);
        } else {
            // not in IBD mode, so drop the status receiver
            let _ = sync.block_queue_consume_status_recv.take();
        }

        sync
    }

    /// Get shared state
    pub fn shared(&self) -> &Arc<SyncShared> {
        &self.shared
    }

    fn try_process<'r>(
        &mut self,
        nc: &dyn CKBProtocolContext,
        peer: PeerIndex,
        message: packed::SyncMessageUnionReader<'r>,
    ) -> Status {
        match message {
            packed::SyncMessageUnionReader::GetHeaders(reader) => {
                GetHeadersProcess::new(reader, self, peer, nc).execute()
            }
            packed::SyncMessageUnionReader::SendHeaders(reader) => {
                HeadersProcess::new(reader, self, peer, nc).execute()
            }
            packed::SyncMessageUnionReader::GetBlocks(reader) => {
                GetBlocksProcess::new(reader, self, peer, nc).execute()
            }
            packed::SyncMessageUnionReader::SendBlock(reader) => {
                if reader.check_data() {
                    BlockProcess::new(reader, self, peer).execute()
                } else {
                    StatusCode::ProtocolMessageIsMalformed.with_context("SendBlock is invalid")
                }
            }
            packed::SyncMessageUnionReader::InIBD(_) => InIBDProcess::new(self, peer, nc).execute(),
            _ => StatusCode::ProtocolMessageIsMalformed.with_context("unexpected sync message"),
        }
    }

    fn process<'r>(
        &mut self,
        nc: &dyn CKBProtocolContext,
        peer: PeerIndex,
        message: packed::SyncMessageUnionReader<'r>,
    ) {
        let item_name = message.item_name();
        let item_bytes = message.as_slice().len() as u64;
        let item_id = message.item_id();
        let status = self.try_process(nc, peer, message);
        Self::metrics_block_process(item_bytes, item_id, &status);
        Self::post_block_process(nc, peer, item_name, status)
    }

    fn metrics_block_process(item_bytes_length: u64, item_id: Number, status: &Status) {
        metrics!(
            counter,
            "ckb.messages_bytes",
            item_bytes_length,
            "direction" => "in",
            "protocol_id" => SupportProtocols::Sync.protocol_id().value().to_string(),
            "item_id" =>  item_id.to_string(),
            "status" => (status.code() as u16).to_string(),
        );
    }

    fn post_block_process(
        nc: &dyn CKBProtocolContext,
        peer: PeerIndex,
        item_name: &str,
        status: Status,
    ) {
        if let Some(ban_time) = status.should_ban() {
            error!(
                "receive {} from {}, ban {:?} for {}",
                item_name, peer, ban_time, status
            );
            nc.ban_peer(peer, ban_time, status.to_string());
        } else if status.should_warn() {
            warn!("receive {} from {}, {}", item_name, peer, status);
        } else if !status.is_ok() {
            debug!("receive {} from {}, {}", item_name, peer, status);
        }
    }

    /// Get peers info
    pub fn peers(&self) -> &Peers {
        self.shared().state().peers()
    }

    fn better_tip_header(&self) -> core::HeaderView {
        let (header, total_difficulty) = {
            let active_chain = self.shared.active_chain();
            (
                active_chain.tip_header(),
                active_chain.total_difficulty().to_owned(),
            )
        };
        let best_known = self.shared.state().shared_best_header();
        // is_better_chain
        if total_difficulty > *best_known.total_difficulty() {
            header
        } else {
            best_known.into_inner()
        }
    }

    /// Process a new block sync from other peer
    //TODO: process block which we don't request
    pub fn process_new_block(&self, block: core::BlockView) -> Result<bool, CKBError> {
        let block_hash = block.hash();
        let status = self.shared.active_chain().get_block_status(&block_hash);
        // NOTE: Filtering `BLOCK_STORED` but not `BLOCK_RECEIVED`, is for avoiding
        // stopping synchronization even when orphan_pool maintains dirty items by bugs.
        if status.contains(BlockStatus::BLOCK_STORED) {
            debug!("block {} already stored", block_hash);
            Ok(false)
        } else if status.contains(BlockStatus::HEADER_VALID) {
            self.shared.insert_new_block(&self.chain, Arc::new(block))
        } else {
            debug!(
                "Synchronizer process_new_block unexpected status {:?} {}",
                status, block_hash,
            );
            // TODO which error should we return?
            Ok(false)
        }
    }

    /// Get blocks to fetch
    pub fn get_blocks_to_fetch(
        &self,
        peer: PeerIndex,
        ibd: IBDState,
    ) -> Option<Vec<Vec<packed::Byte32>>> {
        BlockFetcher::new(self, peer, ibd).fetch()
    }

    pub(crate) fn on_connected(&self, nc: &dyn CKBProtocolContext, peer: PeerIndex) {
        let (is_outbound, is_whitelist) = nc
            .get_peer(peer)
            .map(|peer| (peer.is_outbound(), peer.is_whitelist))
            .unwrap_or((false, false));

        self.peers().sync_connected(peer, is_outbound, is_whitelist);
    }

    /// Regularly check and eject some nodes that do not respond in time
    //   - If at timeout their best known block now has more work than our tip
    //     when the timeout was set, then either reset the timeout or clear it
    //     (after comparing against our current tip's work)
    //   - If at timeout their best known block still has less work than our
    //     tip did when the timeout was set, then send a getheaders message,
    //     and set a shorter timeout, HEADERS_RESPONSE_TIME seconds in future.
    //     If their best known block is still behind when that new timeout is
    //     reached, disconnect.
    pub fn eviction(&self, nc: &dyn CKBProtocolContext) {
        let active_chain = self.shared.active_chain();
        let mut eviction = Vec::new();
        let better_tip_header = self.better_tip_header();
        for mut kv_pair in self.peers().state.iter_mut() {
            let (peer, state) = kv_pair.pair_mut();
            let now = unix_time_as_millis();

            if let Some(ref mut controller) = state.headers_sync_controller {
                let better_tip_ts = better_tip_header.timestamp();
                if let Some(is_timeout) = controller.is_timeout(better_tip_ts, now) {
                    if is_timeout {
                        eviction.push(*peer);
                        continue;
                    }
                } else {
                    active_chain.send_getheaders_to_peer(nc, *peer, &better_tip_header);
                }
            }

            if state.peer_flags.is_outbound {
                let best_known_header = state.best_known_header.as_ref();
                let (tip_header, local_total_difficulty) = {
                    (
                        active_chain.tip_header().to_owned(),
                        active_chain.total_difficulty().to_owned(),
                    )
                };
                if best_known_header.map(HeaderView::total_difficulty)
                    >= Some(&local_total_difficulty)
                {
                    if state.chain_sync.timeout != 0 {
                        state.chain_sync.timeout = 0;
                        state.chain_sync.work_header = None;
                        state.chain_sync.total_difficulty = None;
                        state.chain_sync.sent_getheaders = false;
                    }
                } else if state.chain_sync.timeout == 0
                    || (best_known_header.is_some()
                        && best_known_header.map(HeaderView::total_difficulty)
                            >= state.chain_sync.total_difficulty.as_ref())
                {
                    // Our best block known by this peer is behind our tip, and we're either noticing
                    // that for the first time, OR this peer was able to catch up to some earlier point
                    // where we checked against our tip.
                    // Either way, set a new timeout based on current tip.
                    state.chain_sync.timeout = now + CHAIN_SYNC_TIMEOUT;
                    state.chain_sync.work_header = Some(tip_header);
                    state.chain_sync.total_difficulty = Some(local_total_difficulty);
                    state.chain_sync.sent_getheaders = false;
                } else if state.chain_sync.timeout > 0 && now > state.chain_sync.timeout {
                    // No evidence yet that our peer has synced to a chain with work equal to that
                    // of our tip, when we first detected it was behind. Send a single getheaders
                    // message to give the peer a chance to update us.
                    if state.chain_sync.sent_getheaders {
                        if state.peer_flags.is_protect || state.peer_flags.is_whitelist {
                            if state.sync_started() {
                                self.shared().state().suspend_sync(state);
                            }
                        } else {
                            eviction.push(*peer);
                        }
                    } else {
                        state.chain_sync.sent_getheaders = true;
                        state.chain_sync.timeout = now + EVICTION_HEADERS_RESPONSE_TIME;
                        active_chain.send_getheaders_to_peer(
                            nc,
                            *peer,
                            &state
                                .chain_sync
                                .work_header
                                .clone()
                                .expect("work_header be assigned"),
                        );
                    }
                }
            }
        }
        for peer in eviction {
            info!("timeout eviction peer={}", peer);
            if let Err(err) = nc.disconnect(peer, "sync timeout eviction") {
                debug!("synchronizer disconnect error: {:?}", err);
            }
        }
    }

    fn start_sync_headers(&self, nc: &dyn CKBProtocolContext) {
        let now = unix_time_as_millis();
        let active_chain = self.shared.active_chain();
        let ibd = active_chain.is_initial_block_download();
        let peers: Vec<PeerIndex> = self
            .peers()
            .state
            .iter()
            .filter(|kv_pair| kv_pair.value().can_start_sync(now, ibd))
            .map(|kv_pair| *kv_pair.key())
            .collect();

        if peers.is_empty() {
            return;
        }

        let tip = self.better_tip_header();

        for peer in peers {
            // Only sync with 1 peer if we're in IBD
            if self
                .shared()
                .state()
                .n_sync_started()
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |x| {
                    if ibd && x != 0 {
                        None
                    } else {
                        Some(x + 1)
                    }
                })
                .is_err()
            {
                break;
            }
            {
                if let Some(mut peer_state) = self.peers().state.get_mut(&peer) {
                    peer_state.start_sync(HeadersSyncController::from_header(&tip));
                }
            }

            debug!("start sync peer={}", peer);
            active_chain.send_getheaders_to_peer(nc, peer, &tip);
        }
    }

    fn get_peers_to_fetch(
        &self,
        ibd: IBDState,
        disconnect_list: &HashSet<PeerIndex>,
    ) -> Vec<PeerIndex> {
        trace!("poll find_blocks_to_fetch select peers");
        let state = &self
            .shared
            .state()
            .read_inflight_blocks()
            .download_schedulers;
        let mut peers: Vec<PeerIndex> = self
            .peers()
            .state
            .iter()
            .filter(|kv_pair| {
                let (id, state) = kv_pair.pair();
                if disconnect_list.contains(id) {
                    return false;
                };
                match ibd {
                    IBDState::In => {
                        state.peer_flags.is_outbound
                            || state.peer_flags.is_whitelist
                            || state.peer_flags.is_protect
                    }
                    IBDState::Out => state.started_or_tip_synced(),
                }
            })
            .map(|kv_pair| *kv_pair.key())
            .collect();
        peers.sort_by_key(|id| {
            ::std::cmp::Reverse(
                state
                    .get(id)
                    .map_or(INIT_BLOCKS_IN_TRANSIT_PER_PEER, |d| d.task_count()),
            )
        });
        peers
    }

    fn find_blocks_to_fetch(&mut self, nc: &dyn CKBProtocolContext, ibd: IBDState) {
        let tip = self.shared.active_chain().tip_number();

        let disconnect_list = {
            let mut list = self.shared().state().write_inflight_blocks().prune(tip);
            if let IBDState::In = ibd {
                // best known < tip and in IBD state, and unknown list is empty,
                // these node can be disconnect
                list.extend(
                    self.shared
                        .state()
                        .peers()
                        .get_best_known_less_than_tip_and_unknown_empty(tip),
                )
            };
            list
        };

        for peer in disconnect_list.iter() {
            // It is not forbidden to evict protected nodes:
            // - First of all, this node is not designated by the user for protection,
            //   but is connected randomly. It does not represent the will of the user
            // - Secondly, in the synchronization phase, the nodes with zero download tasks are
            //   retained, apart from reducing the download efficiency, there is no benefit.
            if self
                .peers()
                .get_flag(*peer)
                .map(|flag| flag.is_whitelist)
                .unwrap_or(false)
            {
                continue;
            }
            if let Err(err) = nc.disconnect(*peer, "sync disconnect") {
                debug!("synchronizer disconnect error: {:?}", err);
            }
        }

        // fetch use a lot of cpu time, especially in ibd state
        // so, the fetch function use another thread
        match nc.p2p_control() {
            Some(raw) => match self.fetch_channel {
                Some(ref sender) => {
                    if !sender.is_full() {
                        let peers = self.get_peers_to_fetch(ibd, &disconnect_list);
                        let _ignore = sender.try_send(FetchCMD::Fetch((peers, ibd)));
                    }
                }
                None => {
                    let p2p_control = raw.clone();
                    let (sender, recv) = channel::bounded(2);
                    let peers = self.get_peers_to_fetch(ibd, &disconnect_list);
                    sender.send(FetchCMD::Fetch((peers, ibd))).unwrap();
                    self.fetch_channel = Some(sender);
                    let thread = ::std::thread::Builder::new();
                    let number = self.shared.state().shared_best_header_ref().number();
                    let sync = self.clone();
                    thread
                        .name("BlockDownload".to_string())
                        .spawn(move || {
                            BlockFetchCMD {
                                sync,
                                p2p_control,
                                recv,
                                number,
                                can_start: CanStart::MinWorkNotReach,
                            }
                            .run();
                        })
                        .expect("download thread can't start");
                }
            },
            None => {
                for peer in self.get_peers_to_fetch(ibd, &disconnect_list) {
                    if let Some(fetch) = self.get_blocks_to_fetch(peer, ibd) {
                        for item in fetch {
                            self.send_getblocks(item, nc, peer);
                        }
                    }
                }
            }
        }
    }

    fn send_getblocks(
        &self,
        v_fetch: Vec<packed::Byte32>,
        nc: &dyn CKBProtocolContext,
        peer: PeerIndex,
    ) {
        let content = packed::GetBlocks::new_builder()
            .block_hashes(v_fetch.clone().pack())
            .build();
        let message = packed::SyncMessage::new_builder().set(content).build();

        debug!("send_getblocks len={:?} to peer={}", v_fetch.len(), peer);
        let _status = send_message_to(nc, peer, &message);
    }
}

#[async_trait]
impl CKBProtocolHandler for Synchronizer {
    async fn init(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>) {
        // NOTE: 100ms is what bitcoin use.
        nc.set_notify(SYNC_NOTIFY_INTERVAL, SEND_GET_HEADERS_TOKEN)
            .await
            .expect("set_notify at init is ok");
        nc.set_notify(SYNC_NOTIFY_INTERVAL, TIMEOUT_EVICTION_TOKEN)
            .await
            .expect("set_notify at init is ok");
        nc.set_notify(IBD_BLOCK_FETCH_INTERVAL, IBD_BLOCK_FETCH_TOKEN)
            .await
            .expect("set_notify at init is ok");
        nc.set_notify(NOT_IBD_BLOCK_FETCH_INTERVAL, NOT_IBD_BLOCK_FETCH_TOKEN)
            .await
            .expect("set_notify at init is ok");
        nc.set_notify(Duration::from_secs(2), NO_PEER_CHECK_TOKEN)
            .await
            .expect("set_notify at init is ok");
    }

    async fn received(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        data: Bytes,
    ) {
        let msg = match packed::SyncMessageReader::from_compatible_slice(&data) {
            Ok(msg) => {
                let item = msg.to_enum();
                if let packed::SyncMessageUnionReader::SendBlock(ref reader) = item {
                    if reader.count_extra_fields() > 1 {
                        info!(
                            "Peer {} sends us a malformed message: \
                             too many fields in SendBlock",
                            peer_index
                        );
                        nc.ban_peer(
                            peer_index,
                            BAD_MESSAGE_BAN_TIME,
                            String::from(
                                "send us a malformed message: \
                                 too many fields in SendBlock",
                            ),
                        );
                        return;
                    } else {
                        item
                    }
                } else {
                    match packed::SyncMessageReader::from_slice(&data) {
                        Ok(msg) => msg.to_enum(),
                        _ => {
                            info!(
                                "Peer {} sends us a malformed message: \
                                 too many fields",
                                peer_index
                            );
                            nc.ban_peer(
                                peer_index,
                                BAD_MESSAGE_BAN_TIME,
                                String::from(
                                    "send us a malformed message: \
                                     too many fields",
                                ),
                            );
                            return;
                        }
                    }
                }
            }
            _ => {
                info!("Peer {} sends us a malformed message", peer_index);
                nc.ban_peer(
                    peer_index,
                    BAD_MESSAGE_BAN_TIME,
                    String::from("send us a malformed message"),
                );
                return;
            }
        };

        debug!("received msg {} from {}", msg.item_name(), peer_index);
        #[cfg(feature = "with_sentry")]
        {
            let sentry_hub = sentry::Hub::current();
            let _scope_guard = sentry_hub.push_scope();
            sentry_hub.configure_scope(|scope| {
                scope.set_tag("p2p.protocol", "synchronizer");
                scope.set_tag("p2p.message", msg.item_name());
            });
        }

        let start_time = Instant::now();
        tokio::task::block_in_place(|| self.process(nc.as_ref(), peer_index, msg));
        debug!(
            "process message={}, peer={}, cost={:?}",
            msg.item_name(),
            peer_index,
            Instant::now().saturating_duration_since(start_time),
        );
    }

    async fn connected(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        _version: &str,
    ) {
        info!("SyncProtocol.connected peer={}", peer_index);
        self.on_connected(nc.as_ref(), peer_index);
    }

    async fn disconnected(
        &mut self,
        _nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
    ) {
        info!("SyncProtocol.disconnected peer={}", peer_index);
        let sync_state = self.shared().state();
        sync_state.disconnected(peer_index);
    }

    async fn notify(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>, token: u64) {
        if !self.peers().state.is_empty() {
            let start_time = Instant::now();
            trace!("start notify token={}", token);
            match token {
                SEND_GET_HEADERS_TOKEN => {
                    self.start_sync_headers(nc.as_ref());
                }
                IBD_BLOCK_FETCH_TOKEN => {
                    if self.shared.active_chain().is_initial_block_download() {
                        self.find_blocks_to_fetch(nc.as_ref(), IBDState::In);
                    } else {
                        {
                            self.shared.state().write_inflight_blocks().adjustment = false;
                        }
                        self.shared.state().peers().clear_unknown_list();
                        if nc.remove_notify(IBD_BLOCK_FETCH_TOKEN).await.is_err() {
                            trace!("remove ibd block fetch fail");
                        }
                    }
                }
                NOT_IBD_BLOCK_FETCH_TOKEN => {
                    if !self.shared.active_chain().is_initial_block_download() {
                        self.find_blocks_to_fetch(nc.as_ref(), IBDState::Out);
                    }
                }
                TIMEOUT_EVICTION_TOKEN => {
                    self.eviction(nc.as_ref());
                }
                // Here is just for NO_PEER_CHECK_TOKEN token, only handle it when there is no peer.
                _ => {}
            }

            trace!(
                "finished notify token={} cost={:?}",
                token,
                Instant::now().saturating_duration_since(start_time)
            );
        } else if token == NO_PEER_CHECK_TOKEN {
            debug!("no peers connected");
        }
    }

    async fn poll(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>) -> Option<()> {
        self.block_queue_consume_status_recv.as_ref()?;

        if let Some((status, send_block_msg_info)) = self
            .block_queue_consume_status_recv
            .as_mut()
            .unwrap()
            .next()
            .await
        {
            Self::post_block_process(
                nc.as_ref(),
                send_block_msg_info.peer,
                &send_block_msg_info.item_name,
                status,
            );
            return Some(());
        }

        None
    }
}
