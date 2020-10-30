mod block_fetcher;
mod block_process;
mod get_blocks_process;
mod get_headers_process;
mod headers_process;
mod in_ibd_process;

use self::block_fetcher::BlockFetcher;
use self::block_process::BlockProcess;
use self::get_blocks_process::GetBlocksProcess;
use self::get_headers_process::GetHeadersProcess;
use self::headers_process::HeadersProcess;
use self::in_ibd_process::InIBDProcess;
use crate::block_status::BlockStatus;
use crate::types::{HeaderView, HeadersSyncController, IBDState, PeerFlags, Peers, SyncShared};
use crate::{
    Status, StatusCode, BAD_MESSAGE_BAN_TIME, CHAIN_SYNC_TIMEOUT, EVICTION_HEADERS_RESPONSE_TIME,
    MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT,
};
use ckb_chain::chain::ChainController;
use ckb_channel as channel;
use ckb_logger::{debug, error, info, trace, warn};
use ckb_metrics::metrics;
use ckb_network::{
    bytes::Bytes, CKBProtocolContext, CKBProtocolHandler, PeerIndex, ServiceControl,
    SupportProtocols,
};
use ckb_types::core::BlockNumber;
use ckb_types::{core, packed, prelude::*};
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const SEND_GET_HEADERS_TOKEN: u64 = 0;
pub const IBD_BLOCK_FETCH_TOKEN: u64 = 1;
pub const NOT_IBD_BLOCK_FETCH_TOKEN: u64 = 2;
pub const TIMEOUT_EVICTION_TOKEN: u64 = 3;
pub const NO_PEER_CHECK_TOKEN: u64 = 255;

const SYNC_NOTIFY_INTERVAL: Duration = Duration::from_millis(200);
const IBD_BLOCK_FETCH_INTERVAL: Duration = Duration::from_millis(40);
const NOT_IBD_BLOCK_FETCH_INTERVAL: Duration = Duration::from_millis(200);

enum FetchCMD {
    Fetch(Vec<PeerIndex>),
}

struct BlockFetchCMD {
    sync: Synchronizer,
    p2p_control: ServiceControl,
    recv: channel::Receiver<FetchCMD>,
    can_start: bool,
    number: BlockNumber,
}

impl BlockFetchCMD {
    fn run(&mut self) {
        while let Ok(cmd) = self.recv.recv() {
            match cmd {
                FetchCMD::Fetch(peers) => {
                    if self.can_start() {
                        for peer in peers {
                            if let Some(fetch) =
                                BlockFetcher::new(&self.sync, peer, IBDState::In).fetch()
                            {
                                for item in fetch {
                                    BlockFetchCMD::send_getblocks(item, &self.p2p_control, peer);
                                }
                            }
                        }
                    } else {
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
                                self.sync.shared.consensus().min_chain_work
                            );
                        }
                    }
                }
            }
        }
    }

    fn can_start(&mut self) -> bool {
        if self.can_start {
            true
        } else {
            self.can_start = self
                .sync
                .shared
                .state()
                .shared_best_header_ref()
                .is_better_than(&self.sync.shared.consensus().min_chain_work);
            self.can_start
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
        crate::synchronizer::metrics_counter_send(message.to_enum().item_name());
    }
}

/// TODO(doc): @driftluo
#[derive(Clone)]
pub struct Synchronizer {
    chain: ChainController,
    /// TODO(doc): @driftluo
    pub shared: Arc<SyncShared>,
    fetch_channel: Option<channel::Sender<FetchCMD>>,
}

impl Synchronizer {
    /// TODO(doc): @driftluo
    pub fn new(chain: ChainController, shared: Arc<SyncShared>) -> Synchronizer {
        Synchronizer {
            chain,
            shared,
            fetch_channel: None,
        }
    }

    /// TODO(doc): @driftluo
    pub fn shared(&self) -> &Arc<SyncShared> {
        &self.shared
    }

    fn try_process<'r>(
        &self,
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
        &self,
        nc: &dyn CKBProtocolContext,
        peer: PeerIndex,
        message: packed::SyncMessageUnionReader<'r>,
    ) {
        let item_name = message.item_name();
        let status = self.try_process(nc, peer, message);

        metrics!(counter, "ckb-net.received", 1, "action" => "sync", "item" => item_name.to_owned());
        if !status.is_ok() {
            metrics!(counter, "ckb-net.status", 1, "action" => "sync", "status" => status.tag());
        }

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

    /// TODO(doc): @driftluo
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

    /// TODO(doc): @driftluo
    //TODO: process block which we don't request
    pub fn process_new_block(&self, block: core::BlockView) -> Result<bool, FailureError> {
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

    /// TODO(doc): @driftluo
    pub fn get_blocks_to_fetch(
        &self,
        peer: PeerIndex,
        ibd: IBDState,
    ) -> Option<Vec<Vec<packed::Byte32>>> {
        BlockFetcher::new(&self, peer, ibd).fetch()
    }

    fn on_connected(&self, nc: &dyn CKBProtocolContext, peer: PeerIndex) {
        let (is_outbound, is_whitelist) = nc
            .get_peer(peer)
            .map(|peer| (peer.is_outbound(), peer.is_whitelist))
            .unwrap_or((false, false));

        let sync_state = self.shared().state();
        let protect_outbound = is_outbound
            && sync_state
                .n_protected_outbound_peers()
                .load(Ordering::Acquire)
                < MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT;

        if protect_outbound {
            sync_state
                .n_protected_outbound_peers()
                .fetch_add(1, Ordering::Release);
        }

        self.peers().on_connected(
            peer,
            PeerFlags {
                is_outbound,
                is_whitelist,
                is_protect: protect_outbound,
            },
        );
    }

    /// TODO(doc): @driftluo
    //   - If at timeout their best known block now has more work than our tip
    //     when the timeout was set, then either reset the timeout or clear it
    //     (after comparing against our current tip's work)
    //   - If at timeout their best known block still has less work than our
    //     tip did when the timeout was set, then send a getheaders message,
    //     and set a shorter timeout, HEADERS_RESPONSE_TIME seconds in future.
    //     If their best known block is still behind when that new timeout is
    //     reached, disconnect.
    pub fn eviction(&self, nc: &dyn CKBProtocolContext) {
        let mut peer_states = self.peers().state.write();
        let active_chain = self.shared.active_chain();
        let mut eviction = Vec::new();
        let better_tip_header = self.better_tip_header();
        for (peer, state) in peer_states.iter_mut() {
            let now = unix_time_as_millis();

            if let Some(ref mut controller) = state.headers_sync_controller {
                let better_tip_ts = better_tip_header.timestamp();
                if let Some(is_timeout) = controller.is_timeout(better_tip_ts, now) {
                    if is_timeout && !state.disconnect {
                        eviction.push(*peer);
                        state.disconnect = true;
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
                            if state.sync_started {
                                self.shared().state().suspend_sync(state);
                            }
                        } else {
                            eviction.push(*peer);
                            state.disconnect = true;
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
            .read()
            .iter()
            .filter(|(_, state)| state.can_sync(now, ibd))
            .map(|(peer_id, _)| peer_id)
            .cloned()
            .collect();

        if peers.is_empty() {
            return;
        }

        let tip = self.better_tip_header();

        for peer in peers {
            // Only sync with 1 peer if we're in IBD
            if ibd
                && self
                    .shared()
                    .state()
                    .n_sync_started()
                    .load(Ordering::Acquire)
                    != 0
            {
                break;
            }
            {
                let mut state = self.peers().state.write();
                if let Some(peer_state) = state.get_mut(&peer) {
                    if !peer_state.sync_started {
                        peer_state.start_sync(HeadersSyncController::from_header(&tip));
                        self.shared()
                            .state()
                            .n_sync_started()
                            .fetch_add(1, Ordering::Release);
                    }
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
            .read()
            .iter()
            .filter(|(id, state)| {
                if disconnect_list.contains(id) {
                    return false;
                };
                match ibd {
                    IBDState::In => {
                        state.peer_flags.is_outbound
                            || state.peer_flags.is_whitelist
                            || state.peer_flags.is_protect
                    }
                    IBDState::Out => state.sync_started,
                }
            })
            .map(|(peer_id, _)| peer_id)
            .cloned()
            .collect();
        peers.sort_by_key(|id| {
            ::std::cmp::Reverse(
                state
                    .get(id)
                    .map_or(crate::INIT_BLOCKS_IN_TRANSIT_PER_PEER, |d| d.task_count()),
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
            if self
                .peers()
                .get_flag(*peer)
                .map(|flag| flag.is_whitelist || flag.is_protect)
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
                        let _ignore = sender.try_send(FetchCMD::Fetch(peers));
                    }
                }
                None => {
                    let p2p_control = raw.clone();
                    let sync = self.clone();
                    let (sender, recv) = channel::bounded(2);
                    let peers = self.get_peers_to_fetch(ibd, &disconnect_list);
                    sender.send(FetchCMD::Fetch(peers)).unwrap();
                    self.fetch_channel = Some(sender);
                    let thread = ::std::thread::Builder::new();
                    let number = self.shared.state().shared_best_header_ref().number();
                    thread
                        .name("BlockDownload".to_string())
                        .spawn(move || {
                            BlockFetchCMD {
                                sync,
                                p2p_control,
                                recv,
                                number,
                                can_start: false,
                            }
                            .run();
                        })
                        .expect("donwload thread can't start");
                }
            },
            _ => {
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
        if let Err(err) = nc.send_message_to(peer, message.as_bytes()) {
            debug!("synchronizer send GetBlocks error: {:?}", err);
        }
        crate::synchronizer::metrics_counter_send(message.to_enum().item_name());
    }
}

impl CKBProtocolHandler for Synchronizer {
    fn init(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>) {
        // NOTE: 100ms is what bitcoin use.
        nc.set_notify(SYNC_NOTIFY_INTERVAL, SEND_GET_HEADERS_TOKEN)
            .expect("set_notify at init is ok");
        nc.set_notify(SYNC_NOTIFY_INTERVAL, TIMEOUT_EVICTION_TOKEN)
            .expect("set_notify at init is ok");
        nc.set_notify(IBD_BLOCK_FETCH_INTERVAL, IBD_BLOCK_FETCH_TOKEN)
            .expect("set_notify at init is ok");
        nc.set_notify(NOT_IBD_BLOCK_FETCH_INTERVAL, NOT_IBD_BLOCK_FETCH_TOKEN)
            .expect("set_notify at init is ok");
        nc.set_notify(Duration::from_secs(2), NO_PEER_CHECK_TOKEN)
            .expect("set_notify at init is ok");
    }

    fn received(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        data: Bytes,
    ) {
        let msg = match packed::SyncMessage::from_slice(&data) {
            Ok(msg) => msg.to_enum(),
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
        let sentry_hub = sentry::Hub::current();
        let _scope_guard = sentry_hub.push_scope();
        sentry_hub.configure_scope(|scope| {
            scope.set_tag("p2p.protocol", "synchronizer");
            scope.set_tag("p2p.message", msg.item_name());
        });

        let start_time = Instant::now();
        self.process(nc.as_ref(), peer_index, msg.as_reader());
        debug!(
            "process message={}, peer={}, cost={:?}",
            msg.item_name(),
            peer_index,
            start_time.elapsed(),
        );
    }

    fn connected(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        _version: &str,
    ) {
        info!("SyncProtocol.connected peer={}", peer_index);
        self.on_connected(nc.as_ref(), peer_index);
    }

    fn disconnected(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>, peer_index: PeerIndex) {
        let sync_state = self.shared().state();
        if let Some(peer_state) = sync_state.disconnected(peer_index) {
            info!("SyncProtocol.disconnected peer={}", peer_index);

            if peer_state.sync_started {
                // It shouldn't happen
                // fetch_sub wraps around on overflow, we still check manually
                // panic here to prevent some bug be hidden silently.
                assert_ne!(
                    sync_state.n_sync_started().fetch_sub(1, Ordering::Release),
                    0,
                    "n_sync_started overflow when disconnects"
                );
            }

            // Protection node disconnected
            if peer_state.peer_flags.is_protect {
                assert_ne!(
                    sync_state
                        .n_protected_outbound_peers()
                        .fetch_sub(1, Ordering::Release),
                    0,
                    "n_protected_outbound_peers overflow when disconnects"
                );
            }
        }
    }

    fn notify(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>, token: u64) {
        if !self.peers().state.read().is_empty() {
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
                        if nc.remove_notify(IBD_BLOCK_FETCH_TOKEN).is_err() {
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
                start_time.elapsed()
            );
        } else if token == NO_PEER_CHECK_TOKEN {
            debug!("no peers connected");
        }
    }
}

#[cfg(test)]
mod tests {
    use self::block_process::BlockProcess;
    use self::headers_process::HeadersProcess;
    use super::*;
    use crate::{
        types::{HeaderView, HeadersSyncController, PeerState},
        SyncShared, MAX_TIP_AGE,
    };
    use ckb_chain::{chain::ChainService, switch::Switch};
    use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
    use ckb_dao::DaoCalculator;
    use ckb_network::{
        bytes::Bytes, Behaviour, CKBProtocolContext, Peer, PeerId, PeerIndex, ProtocolId,
        SessionType, TargetSession,
    };
    use ckb_shared::{
        shared::{Shared, SharedBuilder},
        Snapshot,
    };
    use ckb_store::ChainStore;
    use ckb_types::{
        core::{
            cell::resolve_transaction, BlockBuilder, BlockNumber, BlockView, EpochExt,
            HeaderBuilder, HeaderView as CoreHeaderView, TransactionBuilder, TransactionView,
        },
        packed::{
            Byte32, CellInput, CellOutputBuilder, Script, SendBlockBuilder, SendHeadersBuilder,
        },
        utilities::difficulty_to_compact,
        U256,
    };
    use ckb_util::Mutex;
    use futures::future::Future;
    use std::{
        collections::{HashMap, HashSet},
        ops::Deref,
        pin::Pin,
        time::Duration,
    };

    fn start_chain(consensus: Option<Consensus>) -> (ChainController, Shared) {
        let mut builder = SharedBuilder::default();

        let consensus = consensus.unwrap_or_else(Default::default);
        builder = builder.consensus(consensus);

        let (shared, table) = builder.build().unwrap();

        let chain_service = ChainService::new(shared.clone(), table);
        let chain_controller = chain_service.start::<&str>(None);

        (chain_controller, shared)
    }

    fn create_cellbase(
        shared: &Shared,
        parent_header: &CoreHeaderView,
        number: BlockNumber,
    ) -> TransactionView {
        let (_, reward) = shared
            .snapshot()
            .finalize_block_reward(parent_header)
            .unwrap();

        let builder = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(number))
            .witness(Script::default().into_witness());
        if number <= shared.consensus().finalization_delay_length() {
            builder.build()
        } else {
            builder
                .output(
                    CellOutputBuilder::default()
                        .capacity(reward.total.pack())
                        .build(),
                )
                .output_data(Bytes::new().pack())
                .build()
        }
    }

    fn gen_synchronizer(chain_controller: ChainController, shared: Shared) -> Synchronizer {
        let shared = Arc::new(SyncShared::new(shared, Default::default()));
        Synchronizer::new(chain_controller, shared)
    }

    fn gen_block(
        shared: &Shared,
        parent_header: &CoreHeaderView,
        epoch: &EpochExt,
        nonce: u128,
    ) -> BlockView {
        let now = 1 + parent_header.timestamp();
        let number = parent_header.number() + 1;
        let cellbase = create_cellbase(shared, parent_header, number);
        let dao = {
            let snapshot: &Snapshot = &shared.snapshot();
            let resolved_cellbase =
                resolve_transaction(cellbase.clone(), &mut HashSet::new(), snapshot, snapshot)
                    .unwrap();
            DaoCalculator::new(shared.consensus(), shared.store())
                .dao_field(&[resolved_cellbase], parent_header)
                .unwrap()
        };

        BlockBuilder::default()
            .transaction(cellbase)
            .parent_hash(parent_header.hash())
            .timestamp(now.pack())
            .epoch(epoch.number_with_fraction(number).pack())
            .number(number.pack())
            .compact_target(epoch.compact_target().pack())
            .nonce(nonce.pack())
            .dao(dao)
            .build()
    }

    fn insert_block(
        chain_controller: &ChainController,
        shared: &Shared,
        nonce: u128,
        number: BlockNumber,
    ) {
        let snapshot = shared.snapshot();
        let parent = snapshot
            .get_block_header(&snapshot.get_block_hash(number - 1).unwrap())
            .unwrap();
        let parent_epoch = snapshot.get_block_epoch(&parent.hash()).unwrap();
        let epoch = snapshot
            .next_epoch_ext(snapshot.consensus(), &parent_epoch, &parent)
            .unwrap_or(parent_epoch);

        let block = gen_block(shared, &parent, &epoch, nonce);

        chain_controller
            .process_block(Arc::new(block))
            .expect("process block ok");
    }

    #[test]
    fn test_locator() {
        let (chain_controller, shared) = start_chain(None);

        let num = 200;
        let index = [
            199, 198, 197, 196, 195, 194, 193, 192, 191, 190, 188, 184, 176, 160, 128, 64,
        ];

        for i in 1..num {
            insert_block(&chain_controller, &shared, u128::from(i), i);
        }

        let synchronizer = gen_synchronizer(chain_controller, shared.clone());

        let locator = synchronizer
            .shared
            .active_chain()
            .get_locator(shared.snapshot().tip_header());

        let mut expect = Vec::new();

        for i in index.iter() {
            expect.push(shared.store().get_block_hash(*i).unwrap());
        }
        //genesis_hash must be the last one
        expect.push(shared.genesis_hash());

        assert_eq!(expect, locator);
    }

    #[test]
    fn test_locate_latest_common_block() {
        let consensus = Consensus::default();
        let (chain_controller1, shared1) = start_chain(Some(consensus.clone()));
        let (chain_controller2, shared2) = start_chain(Some(consensus.clone()));
        let num = 200;

        for i in 1..num {
            insert_block(&chain_controller1, &shared1, u128::from(i), i);
        }

        for i in 1..num {
            insert_block(&chain_controller2, &shared2, u128::from(i + 1), i);
        }

        let synchronizer1 = gen_synchronizer(chain_controller1, shared1.clone());

        let synchronizer2 = gen_synchronizer(chain_controller2, shared2);

        let locator1 = synchronizer1
            .shared
            .active_chain()
            .get_locator(shared1.snapshot().tip_header());

        let latest_common = synchronizer2
            .shared
            .active_chain()
            .locate_latest_common_block(&Byte32::zero(), &locator1[..]);

        assert_eq!(latest_common, Some(0));

        let (chain_controller3, shared3) = start_chain(Some(consensus));

        for i in 1..num {
            let j = if i > 192 { i + 1 } else { i };
            insert_block(&chain_controller3, &shared3, u128::from(j), i);
        }

        let synchronizer3 = gen_synchronizer(chain_controller3, shared3);

        let latest_common3 = synchronizer3
            .shared
            .active_chain()
            .locate_latest_common_block(&Byte32::zero(), &locator1[..]);
        assert_eq!(latest_common3, Some(192));
    }

    #[test]
    fn test_locate_latest_common_block2() {
        let consensus = Consensus::default();
        let (chain_controller1, shared1) = start_chain(Some(consensus.clone()));
        let (chain_controller2, shared2) = start_chain(Some(consensus.clone()));
        let block_number = 200;

        let mut blocks: Vec<BlockView> = Vec::new();
        let mut parent = consensus.genesis_block().header();

        for i in 1..block_number {
            let store = shared1.store();
            let parent_epoch = store.get_block_epoch(&parent.hash()).unwrap();
            let epoch = store
                .next_epoch_ext(shared1.consensus(), &parent_epoch, &parent)
                .unwrap_or(parent_epoch);
            let new_block = gen_block(&shared1, &parent, &epoch, i);
            blocks.push(new_block.clone());

            chain_controller1
                .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
                .expect("process block ok");
            chain_controller2
                .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
                .expect("process block ok");
            parent = new_block.header().to_owned();
        }

        parent = blocks[150].header();
        let fork = parent.number();
        for i in 1..=block_number {
            let store = shared2.store();
            let parent_epoch = store.get_block_epoch(&parent.hash()).unwrap();
            let epoch = store
                .next_epoch_ext(shared2.consensus(), &parent_epoch, &parent)
                .unwrap_or(parent_epoch);
            let new_block = gen_block(&shared2, &parent, &epoch, i + 100);

            chain_controller2
                .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
                .expect("process block ok");
            parent = new_block.header().to_owned();
        }

        let synchronizer1 = gen_synchronizer(chain_controller1, shared1.clone());
        let synchronizer2 = gen_synchronizer(chain_controller2, shared2.clone());
        let locator1 = synchronizer1
            .shared
            .active_chain()
            .get_locator(shared1.snapshot().tip_header());

        let latest_common = synchronizer2
            .shared
            .active_chain()
            .locate_latest_common_block(&Byte32::zero(), &locator1[..])
            .unwrap();

        assert_eq!(
            shared1.snapshot().get_block_hash(fork).unwrap(),
            shared2.snapshot().get_block_hash(fork).unwrap()
        );
        assert!(
            shared1.snapshot().get_block_hash(fork + 1).unwrap()
                != shared2.snapshot().get_block_hash(fork + 1).unwrap()
        );
        assert_eq!(
            shared1.snapshot().get_block_hash(latest_common).unwrap(),
            shared1.snapshot().get_block_hash(fork).unwrap()
        );
    }

    #[test]
    fn test_get_ancestor() {
        let consensus = Consensus::default();
        let (chain_controller, shared) = start_chain(Some(consensus));
        let num = 200;

        for i in 1..num {
            insert_block(&chain_controller, &shared, u128::from(i), i);
        }

        let synchronizer = gen_synchronizer(chain_controller, shared.clone());

        let header = synchronizer
            .shared
            .active_chain()
            .get_ancestor(&shared.snapshot().tip_header().hash(), 100);
        let tip = synchronizer
            .shared
            .active_chain()
            .get_ancestor(&shared.snapshot().tip_header().hash(), 199);
        let noop = synchronizer
            .shared
            .active_chain()
            .get_ancestor(&shared.snapshot().tip_header().hash(), 200);
        assert!(tip.is_some());
        assert!(header.is_some());
        assert!(noop.is_none());
        assert_eq!(tip.unwrap(), shared.snapshot().tip_header().to_owned());
        assert_eq!(
            header.unwrap(),
            shared
                .store()
                .get_block_header(&shared.store().get_block_hash(100).unwrap())
                .unwrap()
        );
    }

    #[test]
    fn test_process_new_block() {
        let consensus = Consensus::default();
        let (chain_controller1, shared1) = start_chain(Some(consensus.clone()));
        let (chain_controller2, shared2) = start_chain(Some(consensus));
        let block_number = 2000;

        let mut blocks: Vec<BlockView> = Vec::new();
        let mut parent = shared1
            .store()
            .get_block_header(&shared1.store().get_block_hash(0).unwrap())
            .unwrap();
        for i in 1..block_number {
            let store = shared1.store();
            let parent_epoch = store.get_block_epoch(&parent.hash()).unwrap();
            let epoch = store
                .next_epoch_ext(shared1.consensus(), &parent_epoch, &parent)
                .unwrap_or(parent_epoch);
            let new_block = gen_block(&shared1, &parent, &epoch, i + 100);

            chain_controller1
                .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
                .expect("process block ok");
            parent = new_block.header().to_owned();
            blocks.push(new_block);
        }
        let synchronizer = gen_synchronizer(chain_controller2, shared2.clone());
        let chain1_last_block = blocks.last().cloned().unwrap();
        blocks.into_iter().for_each(|block| {
            synchronizer
                .shared()
                .insert_new_block(&synchronizer.chain, Arc::new(block))
                .expect("Insert new block failed");
        });
        assert_eq!(&chain1_last_block.header(), shared2.snapshot().tip_header());
    }

    #[test]
    fn test_get_locator_response() {
        let consensus = Consensus::default();
        let (chain_controller, shared) = start_chain(Some(consensus));
        let block_number = 200;

        let mut blocks: Vec<BlockView> = Vec::new();
        let mut parent = shared
            .store()
            .get_block_header(&shared.store().get_block_hash(0).unwrap())
            .unwrap();
        for i in 1..=block_number {
            let store = shared.snapshot();
            let parent_epoch = store.get_block_epoch(&parent.hash()).unwrap();
            let epoch = store
                .next_epoch_ext(shared.consensus(), &parent_epoch, &parent)
                .unwrap_or(parent_epoch);
            let new_block = gen_block(&shared, &parent, &epoch, i + 100);
            blocks.push(new_block.clone());

            chain_controller
                .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
                .expect("process block ok");
            parent = new_block.header().to_owned();
        }

        let synchronizer = gen_synchronizer(chain_controller, shared);

        let headers = synchronizer
            .shared
            .active_chain()
            .get_locator_response(180, &Byte32::zero());

        assert_eq!(headers.first().unwrap(), &blocks[180].header());
        assert_eq!(headers.last().unwrap(), &blocks[199].header());

        for window in headers.windows(2) {
            if let [parent, header] = &window {
                assert_eq!(header.data().raw().parent_hash(), parent.hash());
            }
        }
    }

    #[derive(Clone)]
    struct DummyNetworkContext {
        pub peers: HashMap<PeerIndex, Peer>,
        pub disconnected: Arc<Mutex<HashSet<PeerIndex>>>,
    }

    fn mock_peer_info() -> Peer {
        Peer::new(
            0.into(),
            SessionType::Outbound,
            PeerId::random(),
            "/ip4/127.0.0.1".parse().expect("parse multiaddr"),
            false,
        )
    }

    fn mock_header_view(total_difficulty: u64) -> HeaderView {
        HeaderView::new(
            HeaderBuilder::default().build(),
            U256::from(total_difficulty),
        )
    }

    impl CKBProtocolContext for DummyNetworkContext {
        // Interact with underlying p2p service
        fn set_notify(&self, _interval: Duration, _token: u64) -> Result<(), ckb_network::Error> {
            unimplemented!();
        }

        fn remove_notify(&self, _token: u64) -> Result<(), ckb_network::Error> {
            unimplemented!()
        }

        fn future_task(
            &self,
            _task: Pin<Box<dyn Future<Output = ()> + 'static + Send>>,
            _blocking: bool,
        ) -> Result<(), ckb_network::Error> {
            //            task.await.expect("resolve future task error");
            Ok(())
        }

        fn quick_send_message(
            &self,
            proto_id: ProtocolId,
            peer_index: PeerIndex,
            data: Bytes,
        ) -> Result<(), ckb_network::Error> {
            self.send_message(proto_id, peer_index, data)
        }
        fn quick_send_message_to(
            &self,
            peer_index: PeerIndex,
            data: Bytes,
        ) -> Result<(), ckb_network::Error> {
            self.send_message_to(peer_index, data)
        }
        fn quick_filter_broadcast(
            &self,
            target: TargetSession,
            data: Bytes,
        ) -> Result<(), ckb_network::Error> {
            self.filter_broadcast(target, data)
        }
        fn send_message(
            &self,
            _proto_id: ProtocolId,
            _peer_index: PeerIndex,
            _data: Bytes,
        ) -> Result<(), ckb_network::Error> {
            Ok(())
        }
        fn send_message_to(
            &self,
            _peer_index: PeerIndex,
            _data: Bytes,
        ) -> Result<(), ckb_network::Error> {
            Ok(())
        }
        fn filter_broadcast(
            &self,
            _target: TargetSession,
            _data: Bytes,
        ) -> Result<(), ckb_network::Error> {
            Ok(())
        }
        fn disconnect(&self, peer_index: PeerIndex, _msg: &str) -> Result<(), ckb_network::Error> {
            self.disconnected.lock().insert(peer_index);
            Ok(())
        }
        // Interact with NetworkState
        fn get_peer(&self, peer_index: PeerIndex) -> Option<Peer> {
            self.peers.get(&peer_index).cloned()
        }
        fn with_peer_mut(&self, _peer_index: PeerIndex, _f: Box<dyn FnOnce(&mut Peer)>) {}
        fn connected_peers(&self) -> Vec<PeerIndex> {
            unimplemented!();
        }
        fn report_peer(&self, _peer_index: PeerIndex, _behaviour: Behaviour) {}
        fn ban_peer(&self, _peer_index: PeerIndex, _duration: Duration, _reason: String) {}
        // Other methods
        fn protocol_id(&self) -> ProtocolId {
            unimplemented!();
        }
        fn send_paused(&self) -> bool {
            false
        }
    }

    fn mock_network_context(peer_num: usize) -> DummyNetworkContext {
        let mut peers = HashMap::default();
        for peer in 0..peer_num {
            peers.insert(peer.into(), mock_peer_info());
        }
        DummyNetworkContext {
            peers,
            disconnected: Arc::new(Mutex::new(HashSet::default())),
        }
    }

    #[test]
    fn test_sync_process() {
        let consensus = Consensus::default();
        let (chain_controller1, shared1) = start_chain(Some(consensus.clone()));
        let (chain_controller2, shared2) = start_chain(Some(consensus));
        let num = 200;

        for i in 1..num {
            insert_block(&chain_controller1, &shared1, u128::from(i), i);
        }

        let synchronizer1 = gen_synchronizer(chain_controller1, shared1.clone());

        let locator1 = synchronizer1
            .shared
            .active_chain()
            .get_locator(&shared1.snapshot().tip_header());

        for i in 1..=num {
            let j = if i > 192 { i + 1 } else { i };
            insert_block(&chain_controller2, &shared2, u128::from(j), i);
        }

        let synchronizer2 = gen_synchronizer(chain_controller2, shared2.clone());
        let latest_common = synchronizer2
            .shared
            .active_chain()
            .locate_latest_common_block(&Byte32::zero(), &locator1[..]);
        assert_eq!(latest_common, Some(192));

        let headers = synchronizer2
            .shared
            .active_chain()
            .get_locator_response(192, &Byte32::zero());

        assert_eq!(
            headers.first().unwrap().hash(),
            shared2.store().get_block_hash(193).unwrap()
        );
        assert_eq!(
            headers.last().unwrap().hash(),
            shared2.store().get_block_hash(200).unwrap()
        );

        let sendheaders = SendHeadersBuilder::default()
            .headers(headers.iter().map(|h| h.data()).pack())
            .build();

        let mock_nc = mock_network_context(4);
        let peer1: PeerIndex = 1.into();
        let peer2: PeerIndex = 2.into();
        synchronizer1.on_connected(&mock_nc, peer1);
        synchronizer1.on_connected(&mock_nc, peer2);
        assert_eq!(
            HeadersProcess::new(sendheaders.as_reader(), &synchronizer1, peer1, &mock_nc).execute(),
            Status::ok(),
        );

        let best_known_header = synchronizer1.peers().get_best_known_header(peer1);

        assert_eq!(best_known_header.unwrap().inner(), headers.last().unwrap());

        let blocks_to_fetch = synchronizer1
            .get_blocks_to_fetch(peer1, IBDState::Out)
            .unwrap();

        assert_eq!(
            blocks_to_fetch[0].first().unwrap(),
            &shared2.store().get_block_hash(193).unwrap()
        );
        assert_eq!(
            blocks_to_fetch[0].last().unwrap(),
            &shared2.store().get_block_hash(200).unwrap()
        );

        let mut fetched_blocks = Vec::new();
        for block_hash in &blocks_to_fetch[0] {
            fetched_blocks.push(shared2.store().get_block(block_hash).unwrap());
        }

        for block in &fetched_blocks {
            let block = SendBlockBuilder::default().block(block.data()).build();
            assert_eq!(
                BlockProcess::new(block.as_reader(), &synchronizer1, peer1).execute(),
                Status::ok(),
            );
        }

        // After the above blocks stored, we should remove them from in-flight pool
        synchronizer1
            .shared()
            .state()
            .write_inflight_blocks()
            .remove_by_peer(peer1);

        // Construct a better tip, to trigger fixing last_common_header inside `get_blocks_to_fetch`
        insert_block(&synchronizer2.chain, &shared2, 201u128, 201);
        let headers = vec![synchronizer2.shared.active_chain().tip_header()];
        let sendheaders = SendHeadersBuilder::default()
            .headers(headers.iter().map(|h| h.data()).pack())
            .build();
        assert_eq!(
            HeadersProcess::new(sendheaders.as_reader(), &synchronizer1, peer1, &mock_nc).execute(),
            Status::ok(),
        );

        synchronizer1
            .get_blocks_to_fetch(peer1, IBDState::Out)
            .unwrap();

        let last_common_header2 = synchronizer1.peers().get_last_common_header(peer1).unwrap();
        assert_eq!(
            &last_common_header2.hash(),
            blocks_to_fetch[0].last().unwrap(),
            "last_common_header change because it update during get_blocks_to_fetch",
        );
    }

    #[cfg(not(disable_faketime))]
    #[test]
    fn test_header_sync_timeout() {
        use std::iter::FromIterator;
        let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
        faketime::enable(&faketime_file);

        let (chain_controller, shared) = start_chain(None);

        let synchronizer = gen_synchronizer(chain_controller, shared);

        let network_context = mock_network_context(5);
        faketime::write_millis(&faketime_file, MAX_TIP_AGE * 2).expect("write millis");
        assert!(synchronizer
            .shared
            .active_chain()
            .is_initial_block_download());
        let peers = synchronizer.peers();
        // protect should not effect headers_timeout
        {
            let timeout = HeadersSyncController::new(0, 0, 0, 0, false);
            let not_timeout =
                HeadersSyncController::new(MAX_TIP_AGE * 2, 0, MAX_TIP_AGE * 2, 0, false);

            let mut state = peers.state.write();
            let mut state_0 = PeerState::default();
            state_0.peer_flags.is_protect = true;
            state_0.peer_flags.is_outbound = true;
            state_0.headers_sync_controller = Some(timeout);

            let mut state_1 = PeerState::default();
            state_1.peer_flags.is_outbound = true;
            state_1.headers_sync_controller = Some(timeout);

            let mut state_2 = PeerState::default();
            state_2.peer_flags.is_whitelist = true;
            state_2.peer_flags.is_outbound = true;
            state_2.headers_sync_controller = Some(timeout);

            let mut state_3 = PeerState::default();
            state_3.peer_flags.is_outbound = true;
            state_3.headers_sync_controller = Some(not_timeout);

            state.insert(0.into(), state_0);
            state.insert(1.into(), state_1);
            state.insert(2.into(), state_2);
            state.insert(3.into(), state_3);
        }
        synchronizer.eviction(&network_context);
        let disconnected = network_context.disconnected.lock();
        assert_eq!(
            disconnected.deref(),
            &HashSet::from_iter(vec![0, 1, 2].into_iter().map(Into::into))
        )
    }

    #[cfg(not(disable_faketime))]
    #[test]
    fn test_chain_sync_timeout() {
        use std::iter::FromIterator;
        let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
        faketime::enable(&faketime_file);

        let consensus = Consensus::default();
        let block = BlockBuilder::default()
            .compact_target(difficulty_to_compact(U256::from(3u64)).pack())
            .transaction(consensus.genesis_block().transactions()[0].clone())
            .build();
        let consensus = ConsensusBuilder::default().genesis_block(block).build();

        let (chain_controller, shared) = start_chain(Some(consensus));

        assert_eq!(shared.snapshot().total_difficulty(), &U256::from(3u64));

        let synchronizer = gen_synchronizer(chain_controller, shared.clone());

        let network_context = mock_network_context(7);
        let peers = synchronizer.peers();
        //6 peers do not trigger header sync timeout
        let not_timeout = HeadersSyncController::new(MAX_TIP_AGE * 2, 0, MAX_TIP_AGE * 2, 0, false);
        let sync_protected_peer = 0.into();
        {
            let mut state = peers.state.write();
            let mut state_0 = PeerState::default();
            state_0.peer_flags.is_protect = true;
            state_0.peer_flags.is_outbound = true;
            state_0.headers_sync_controller = Some(not_timeout);

            let mut state_1 = PeerState::default();
            state_1.peer_flags.is_protect = true;
            state_1.peer_flags.is_outbound = true;
            state_1.headers_sync_controller = Some(not_timeout);

            let mut state_2 = PeerState::default();
            state_2.peer_flags.is_protect = true;
            state_2.peer_flags.is_outbound = true;
            state_2.headers_sync_controller = Some(not_timeout);

            let mut state_3 = PeerState::default();
            state_3.peer_flags.is_outbound = true;
            state_3.headers_sync_controller = Some(not_timeout);

            let mut state_4 = PeerState::default();
            state_4.peer_flags.is_outbound = true;
            state_4.headers_sync_controller = Some(not_timeout);

            let mut state_5 = PeerState::default();
            state_5.peer_flags.is_outbound = true;
            state_5.headers_sync_controller = Some(not_timeout);

            let mut state_6 = PeerState::default();
            state_6.peer_flags.is_whitelist = true;
            state_6.peer_flags.is_outbound = true;
            state_6.headers_sync_controller = Some(not_timeout);

            state.insert(0.into(), state_0);
            state.insert(1.into(), state_1);
            state.insert(2.into(), state_2);
            state.insert(3.into(), state_3);
            state.insert(4.into(), state_4);
            state.insert(5.into(), state_5);
            state.insert(6.into(), state_6);
        }
        peers.may_set_best_known_header(0.into(), mock_header_view(1));
        peers.may_set_best_known_header(2.into(), mock_header_view(3));
        peers.may_set_best_known_header(3.into(), mock_header_view(1));
        peers.may_set_best_known_header(5.into(), mock_header_view(3));
        {
            // Protected peer 0 start sync
            peers
                .state
                .write()
                .get_mut(&sync_protected_peer)
                .unwrap()
                .start_sync(not_timeout);
            synchronizer
                .shared()
                .state()
                .n_sync_started()
                .fetch_add(1, Ordering::Release);
        }
        synchronizer.eviction(&network_context);
        {
            let peer_state = peers.state.read();
            // Protected peer 0 still in sync state
            assert_eq!(
                peer_state.get(&sync_protected_peer).unwrap().sync_started,
                true
            );
            assert_eq!(
                synchronizer
                    .shared()
                    .state()
                    .n_sync_started()
                    .load(Ordering::Acquire),
                1
            );

            assert!({ network_context.disconnected.lock().is_empty() });
            // start sync with protected peer
            //protect peer is protected from disconnection
            assert!(peer_state
                .get(&2.into())
                .unwrap()
                .chain_sync
                .work_header
                .is_none());
            // Our best block known by this peer is behind our tip, and we're either noticing
            // that for the first time, OR this peer was able to catch up to some earlier point
            // where we checked against our tip.
            // Either way, set a new timeout based on current tip.
            let (tip, total_difficulty) = {
                let snapshot = shared.snapshot();
                let header = snapshot.tip_header().to_owned();
                let total_difficulty = snapshot.total_difficulty().to_owned();
                (header, total_difficulty)
            };
            assert_eq!(
                peer_state.get(&3.into()).unwrap().chain_sync.work_header,
                Some(tip.clone())
            );
            assert_eq!(
                peer_state
                    .get(&3.into())
                    .unwrap()
                    .chain_sync
                    .total_difficulty,
                Some(total_difficulty.clone())
            );
            assert_eq!(
                peer_state.get(&4.into()).unwrap().chain_sync.work_header,
                Some(tip)
            );
            assert_eq!(
                peer_state
                    .get(&4.into())
                    .unwrap()
                    .chain_sync
                    .total_difficulty,
                Some(total_difficulty)
            );
            for proto_id in &[0usize, 1, 3, 4, 6] {
                assert_eq!(
                    peer_state
                        .get(&(*proto_id).into())
                        .unwrap()
                        .chain_sync
                        .timeout,
                    CHAIN_SYNC_TIMEOUT
                );
            }
        }
        faketime::write_millis(&faketime_file, CHAIN_SYNC_TIMEOUT + 1).expect("write millis");
        synchronizer.eviction(&network_context);
        {
            let peer_state = peers.state.read();
            // No evidence yet that our peer has synced to a chain with work equal to that
            // of our tip, when we first detected it was behind. Send a single getheaders
            // message to give the peer a chance to update us.
            assert!({ network_context.disconnected.lock().is_empty() });
            assert_eq!(
                peer_state.get(&3.into()).unwrap().chain_sync.timeout,
                unix_time_as_millis() + EVICTION_HEADERS_RESPONSE_TIME
            );
            assert_eq!(
                peer_state.get(&4.into()).unwrap().chain_sync.timeout,
                unix_time_as_millis() + EVICTION_HEADERS_RESPONSE_TIME
            );
        }
        faketime::write_millis(
            &faketime_file,
            unix_time_as_millis() + EVICTION_HEADERS_RESPONSE_TIME + 1,
        )
        .expect("write millis");
        synchronizer.eviction(&network_context);
        {
            let peer_state = peers.state.read();
            // Protected peer 0 chain_sync timeout
            assert_eq!(
                peer_state.get(&sync_protected_peer).unwrap().sync_started,
                false
            );
            assert_eq!(
                synchronizer
                    .shared()
                    .state()
                    .n_sync_started()
                    .load(Ordering::Acquire),
                0
            );

            // Peer(3,4) run out of time to catch up!
            let disconnected = network_context.disconnected.lock();
            assert_eq!(
                disconnected.deref(),
                &HashSet::from_iter(vec![3, 4].into_iter().map(Into::into))
            )
        }
    }

    #[test]
    // `peer.last_common_header` represents what's the fork point between the local main-chain
    // and the peer's mani-chain. It may be unmatched with the current state. So we expect that
    // the unmatched last_common_header be fixed during `update_last_common_header`
    fn test_fix_last_common_header() {
        //  M1 -> M2 -> M3 -> M4 -> M5 -> M6 (chain M)
        //              \
        //                \-> F4 -> F5 -> F6 -> F7 (chain F)
        let m_ = |number| format!("M{}", number);
        let f_ = |number| format!("F{}", number);
        let mut graph = HashMap::new();
        let mut graph_exts = HashMap::new();

        let main_tip_number = 6u64;
        let fork_tip_number = 7u64;
        let fork_point = 3u64;

        // Construct M chain
        {
            let (chain, shared) = start_chain(Some(Consensus::default()));
            for number in 1..=main_tip_number {
                insert_block(&chain, &shared, u128::from(number), number);
            }
            for number in 0..=main_tip_number {
                let block_hash = shared.snapshot().get_block_hash(number).unwrap();
                let block = shared.snapshot().get_block(&block_hash).unwrap();
                let block_ext = shared.snapshot().get_block_ext(&block_hash).unwrap();
                graph.insert(m_(number), block);
                graph_exts.insert(m_(number), block_ext);
            }
        }
        // Construct F chain
        {
            let (chain, shared) = start_chain(Some(Consensus::default()));
            for number in 1..=fork_tip_number {
                insert_block(
                    &chain,
                    &shared,
                    u128::from(number % (fork_point + 1)),
                    number,
                );
            }
            for number in 0..=fork_tip_number {
                let block_hash = shared.snapshot().get_block_hash(number).unwrap();
                let block = shared.snapshot().get_block(&block_hash).unwrap();
                let block_ext = shared.snapshot().get_block_ext(&block_hash).unwrap();
                graph.insert(f_(number), block);
                graph_exts.insert(f_(number), block_ext);
            }
        }

        // Local has stored M as main-chain, and memoried the headers of F in `SyncState.header_map`
        let (chain, shared) = start_chain(Some(Consensus::default()));
        let synchronizer = gen_synchronizer(chain, shared);
        for number in 1..=main_tip_number {
            let key = m_(number);
            let block = graph.get(&key).cloned().unwrap();
            synchronizer.chain.process_block(Arc::new(block)).unwrap();
        }
        {
            let nc = mock_network_context(1);
            let peer: PeerIndex = 0.into();
            let fork_headers = (1..=fork_tip_number)
                .map(|number| graph.get(&f_(number)).cloned().unwrap())
                .map(|block| block.header().data())
                .collect::<Vec<_>>();
            let sendheaders = SendHeadersBuilder::default()
                .headers(fork_headers.pack())
                .build();
            synchronizer.on_connected(&nc, peer);
            assert!(
                HeadersProcess::new(sendheaders.as_reader(), &synchronizer, peer, &nc)
                    .execute()
                    .is_ok()
            );
        }

        // vec![(last_common_header, best_known_header, fixed_last_common_header)]
        let cases = vec![
            (None, "M2", Some("M2")),
            (None, "F5", Some("M3")),
            (None, "M5", Some("M5")),
            (Some("M1"), "M5", Some("M1")),
            (Some("M1"), "F7", Some("M1")),
            (Some("M4"), "F7", Some("M3")),
            (Some("F4"), "M6", Some("M3")),
            (Some("F4"), "F7", Some("F4")),
            (Some("F7"), "M6", Some("M3")), // peer reorganize
        ];

        let nc = mock_network_context(cases.len());
        for (case, (last_common, best_known, fix_last_common)) in cases.into_iter().enumerate() {
            let peer: PeerIndex = case.into();
            synchronizer.on_connected(&nc, peer);

            let last_common_header =
                last_common.map(|key| graph.get(key).cloned().unwrap().header());
            let best_known_header = {
                let header = graph.get(best_known).cloned().unwrap().header();
                let total_difficulty = graph_exts
                    .get(best_known)
                    .cloned()
                    .unwrap()
                    .total_difficulty;
                HeaderView::new(header, total_difficulty)
            };
            if let Some(state) = synchronizer
                .shared
                .state()
                .peers()
                .state
                .write()
                .get_mut(&peer)
            {
                state.last_common_header = last_common_header;
                state.best_known_header = Some(best_known_header.clone());
            }

            let expected = fix_last_common.map(|mark| mark.to_string());
            let actual = BlockFetcher::new(&synchronizer, peer, IBDState::In)
                .update_last_common_header(&best_known_header)
                .map(|header| {
                    if graph
                        .get(&m_(header.number()))
                        .map(|b| b.hash() != header.hash())
                        .unwrap_or(false)
                    {
                        f_(header.number())
                    } else {
                        m_(header.number())
                    }
                });
            assert_eq!(
                expected, actual,
                "Case: {}, last_common: {:?}, best_known: {:?}, expected: {:?}, actual: {:?}",
                case, last_common, best_known, expected, actual,
            );
        }
    }
}

pub(self) fn metrics_counter_send(item_name: &str) {
    metrics!(counter, "ckb-net.sent", 1, "action" => "sync", "item" => item_name.to_owned());
}
