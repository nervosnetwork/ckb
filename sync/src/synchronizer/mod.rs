//! CKB node has initial block download phase (IBD mode) like Bitcoin:
//! <https://btcinformation.org/en/glossary/initial-block-download>
//!
//! When CKB node is in IBD mode, it will respond `packed::InIBD` to `GetHeaders` and `GetBlocks` requests
//!
//! And CKB has a headers-first synchronization style like Bitcoin:
//! <https://btcinformation.org/en/glossary/headers-first-sync>
//!
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

use crate::types::{post_sync_process, HeadersSyncController, IBDState, Peers, SyncShared};
use crate::utils::{metric_ckb_message_bytes, send_message_to, MetricDirection};
use crate::{Status, StatusCode};
use ckb_shared::block_status::BlockStatus;

use ckb_chain::{ChainController, RemoteBlock};
use ckb_channel as channel;
use ckb_channel::{select, Receiver};
use ckb_constant::sync::{
    BAD_MESSAGE_BAN_TIME, CHAIN_SYNC_TIMEOUT, EVICTION_HEADERS_RESPONSE_TIME,
    INIT_BLOCKS_IN_TRANSIT_PER_PEER, MAX_TIP_AGE,
};
use ckb_logger::{debug, error, info, trace, warn};
use ckb_metrics::HistogramTimer;
use ckb_network::{
    async_trait, bytes::Bytes, tokio, CKBProtocolContext, CKBProtocolHandler, PeerIndex,
    ServiceControl, SupportProtocols,
};
use ckb_shared::types::HeaderIndexView;
use ckb_stop_handler::{new_crossbeam_exit_rx, register_thread};
use ckb_systemtime::unix_time_as_millis;

#[cfg(test)]
use ckb_types::core;
use ckb_types::{
    core::BlockNumber,
    packed::{self, Byte32},
    prelude::*,
};
use std::{
    collections::HashSet,
    sync::{atomic::Ordering, Arc},
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
    FetchToTarget(BlockNumber),
    Ready,
    MinWorkNotReach,
    AssumeValidNotFound,
}

struct FetchCMD {
    peers: Vec<PeerIndex>,
    ibd_state: IBDState,
}

struct BlockFetchCMD {
    sync_shared: Arc<SyncShared>,
    p2p_control: ServiceControl,
    recv: channel::Receiver<FetchCMD>,
    can_start: CanStart,
    number: BlockNumber,
    start_timestamp: u64,
}

impl BlockFetchCMD {
    fn process_fetch_cmd(&mut self, cmd: FetchCMD) {
        let FetchCMD { peers, ibd_state }: FetchCMD = cmd;

        let fetch_blocks_fn = |cmd: &mut BlockFetchCMD, assume_target: BlockNumber| {
            for peer in peers {
                if ckb_stop_handler::has_received_stop_signal() {
                    return;
                }

                let mut fetch_end: BlockNumber = u64::MAX;
                if assume_target != 0 {
                    fetch_end = assume_target
                }

                if let Some(fetch) =
                    BlockFetcher::new(Arc::clone(&cmd.sync_shared), peer, ibd_state)
                        .fetch(fetch_end)
                {
                    for item in fetch {
                        if ckb_stop_handler::has_received_stop_signal() {
                            return;
                        }
                        BlockFetchCMD::send_getblocks(item, &cmd.p2p_control, peer);
                    }
                }
            }
        };

        match self.can_start() {
            CanStart::FetchToTarget(assume_target) => fetch_blocks_fn(self, assume_target),
            CanStart::Ready => fetch_blocks_fn(self, BlockNumber::MAX),
            CanStart::MinWorkNotReach => {
                let best_known = self.sync_shared.state().shared_best_header_ref();
                let number = best_known.number();
                if number != self.number && (number - self.number) % 10000 == 0 {
                    self.number = number;
                    info!(
                            "The current best known header number: {}, total difficulty: {:#x}. \
                                 Block download minimum requirements: header number: 500_000, total difficulty: {:#x}.",
                            number,
                            best_known.total_difficulty(),
                            self.sync_shared.state().min_chain_work()
                        );
                }
            }
            CanStart::AssumeValidNotFound => {
                let state = self.sync_shared.state();
                let shared = self.sync_shared.shared();
                let best_known = state.shared_best_header_ref();
                let number = best_known.number();
                let assume_valid_target: Byte32 = shared
                    .assume_valid_targets()
                    .as_ref()
                    .and_then(|targets| targets.first())
                    .map(Pack::pack)
                    .expect("assume valid target must exist");

                if number != self.number && (number - self.number) % 10000 == 0 {
                    self.number = number;
                    let remaining_headers_sync_log = self.reaming_headers_sync_log();

                    info!(
                        "best known header {}-{}, \
                                 CKB is syncing to latest Header to find the assume valid target: {}. \
                                 Please wait. {}",
                        number,
                        best_known.hash(),
                        assume_valid_target,
                        remaining_headers_sync_log
                    );
                }
            }
        }
    }

    fn reaming_headers_sync_log(&self) -> String {
        if let Some(remaining_headers_needed) = self.calc_time_need_to_reach_latest_tip_header() {
            format!(
                "Need {} minutes to sync to the latest Header.",
                remaining_headers_needed.as_secs() / 60
            )
        } else {
            "".to_string()
        }
    }

    // Timeline:
    //
    // |-------------------|--------------------------------|------------|---->
    // Genesis  (shared best timestamp)                     |           now
    // |                   |                                |            |
    // |             (Sync point)                  (CKB process start)   |
    // |                   |                                             |
    // |--Synced Part------|------------ Remain to Sync -----------------|
    // |                                                                 |
    // |------------------- CKB Chain Age -------------------------------|
    //
    fn calc_time_need_to_reach_latest_tip_header(&self) -> Option<Duration> {
        let genesis_timestamp = self
            .sync_shared
            .consensus()
            .genesis_block()
            .header()
            .timestamp();
        let shared_best_timestamp = self.sync_shared.state().shared_best_header().timestamp();

        let ckb_process_start_timestamp = self.start_timestamp;

        let now_timestamp = unix_time_as_millis();

        let ckb_chain_age = now_timestamp.checked_sub(genesis_timestamp)?;

        let ckb_process_age = now_timestamp.checked_sub(ckb_process_start_timestamp)?;

        let has_synced_headers_age = shared_best_timestamp.checked_sub(genesis_timestamp)?;

        let ckb_sync_header_speed = has_synced_headers_age.checked_div(ckb_process_age)?;

        let sync_all_headers_timecost = ckb_chain_age.checked_div(ckb_sync_header_speed)?;

        let sync_remaining_headers_needed =
            sync_all_headers_timecost.checked_sub(ckb_process_age)?;

        Some(Duration::from_millis(sync_remaining_headers_needed))
    }

    fn run(&mut self, stop_signal: Receiver<()>) {
        loop {
            select! {
                recv(self.recv) -> msg => {
                    if let Ok(cmd) = msg {
                        self.process_fetch_cmd(cmd)
                    }
                }
                recv(stop_signal) -> _ => {
                    info!("BlockDownload received exit signal, exit now");
                    return;
                }
            }
        }
    }

    fn can_start(&mut self) -> CanStart {
        if let CanStart::Ready = self.can_start {
            return self.can_start;
        }

        let shared = self.sync_shared.shared();
        let state = self.sync_shared.state();

        let min_work_reach = |flag: &mut CanStart| {
            if state.min_chain_work_ready() {
                *flag = CanStart::AssumeValidNotFound;
            }
        };

        let assume_valid_target_find = |flag: &mut CanStart| {
            let mut assume_valid_targets = shared.assume_valid_targets();
            if let Some(ref targets) = *assume_valid_targets {
                if targets.is_empty() {
                    assume_valid_targets.take();
                    *flag = CanStart::Ready;
                    return;
                }
                let first_target = targets
                    .first()
                    .expect("has checked targets is not empty, assume valid target must exist");
                match shared.header_map().get(&first_target.pack()) {
                    Some(header) => {
                        if matches!(*flag, CanStart::FetchToTarget(fetch_target) if fetch_target == header.number())
                        {
                            // BlockFetchCMD has set the fetch target, no need to set it again
                        } else {
                            *flag = CanStart::FetchToTarget(header.number());
                            info!("assume valid target found in header_map; CKB will start fetch blocks to {:?} now", header.number_and_hash());
                        }
                        // Blocks that are no longer in the scope of ibd must be forced to verify
                        if unix_time_as_millis().saturating_sub(header.timestamp()) < MAX_TIP_AGE {
                            assume_valid_targets.take();
                            warn!("the duration gap between 'assume valid target' and 'now' is less than 24h; CKB will ignore the specified assume valid target and do full verification from now on");
                        }
                    }
                    None => {
                        // Best known already not in the scope of ibd, it means target is invalid
                        if unix_time_as_millis()
                            .saturating_sub(state.shared_best_header_ref().timestamp())
                            < MAX_TIP_AGE
                        {
                            warn!("the duration gap between 'shared_best_header' and 'now' is less than 24h, but CKB haven't found the assume valid target in header_map; CKB will ignore the specified assume valid target and do full verification from now on");
                            *flag = CanStart::Ready;
                            assume_valid_targets.take();
                        }
                    }
                }
            } else {
                *flag = CanStart::Ready;
            }
        };

        match self.can_start {
            CanStart::FetchToTarget(_) => {
                assume_valid_target_find(&mut self.can_start);
                self.can_start
            }
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
            debug!("synchronizer sending GetBlocks error: {:?}", err);
        }
    }
}

/// Sync protocol handle
pub struct Synchronizer {
    pub(crate) chain: ChainController,
    /// Sync shared state
    pub shared: Arc<SyncShared>,
    fetch_channel: Option<channel::Sender<FetchCMD>>,
}

impl Synchronizer {
    /// Init sync protocol handle
    ///
    /// This is a runtime sync protocol shared state, and any Sync protocol messages will be processed and forwarded by it
    pub fn new(chain: ChainController, shared: Arc<SyncShared>) -> Synchronizer {
        Synchronizer {
            chain,
            shared,
            fetch_channel: None,
        }
    }

    /// Get shared state
    pub fn shared(&self) -> &Arc<SyncShared> {
        &self.shared
    }

    fn try_process(
        &self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        message: packed::SyncMessageUnionReader<'_>,
    ) -> Status {
        let _trace_timecost: Option<HistogramTimer> = {
            ckb_metrics::handle().map(|handle| {
                handle
                    .ckb_sync_msg_process_duration
                    .with_label_values(&[message.item_name()])
                    .start_timer()
            })
        };

        match message {
            packed::SyncMessageUnionReader::GetHeaders(reader) => {
                GetHeadersProcess::new(reader, self, peer, nc.as_ref()).execute()
            }
            packed::SyncMessageUnionReader::SendHeaders(reader) => {
                HeadersProcess::new(reader, self, peer, nc.as_ref()).execute()
            }
            packed::SyncMessageUnionReader::GetBlocks(reader) => {
                GetBlocksProcess::new(reader, self, peer, nc.as_ref()).execute()
            }
            packed::SyncMessageUnionReader::SendBlock(reader) => {
                if reader.check_data() {
                    BlockProcess::new(reader, self, peer, nc).execute()
                } else {
                    StatusCode::ProtocolMessageIsMalformed.with_context("SendBlock is invalid")
                }
            }
            packed::SyncMessageUnionReader::InIBD(_) => {
                InIBDProcess::new(self, peer, nc.as_ref()).execute()
            }
        }
    }

    fn process(
        &self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        message: packed::SyncMessageUnionReader<'_>,
    ) {
        let item_name = message.item_name();
        let item_bytes = message.as_slice().len() as u64;
        let status = self.try_process(Arc::clone(&nc), peer, message);

        metric_ckb_message_bytes(
            MetricDirection::In,
            &SupportProtocols::Sync.name(),
            item_name,
            Some(status.code()),
            item_bytes,
        );

        post_sync_process(nc.as_ref(), peer, item_name, status);
    }

    /// Get peers info
    pub fn peers(&self) -> &Peers {
        self.shared().state().peers()
    }

    fn better_tip_header(&self) -> HeaderIndexView {
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
            (header, total_difficulty).into()
        } else {
            best_known
        }
    }

    /// Process a new block sync from other peer
    //TODO: process block which we don't request
    pub fn asynchronous_process_remote_block(&self, remote_block: RemoteBlock) {
        let block_hash = remote_block.block.hash();
        let status = self.shared.active_chain().get_block_status(&block_hash);
        // NOTE: Filtering `BLOCK_STORED` but not `BLOCK_RECEIVED`, is for avoiding
        // stopping synchronization even when orphan_pool maintains dirty items by bugs.
        if status.contains(BlockStatus::BLOCK_STORED) {
            error!("Block {} already stored", block_hash);
        } else if status.contains(BlockStatus::HEADER_VALID) {
            self.shared.accept_remote_block(&self.chain, remote_block);
        } else {
            debug!(
                "Synchronizer process_new_block unexpected status {:?} {}",
                status, block_hash,
            );
            // TODO which error should we return?
        }
    }

    #[cfg(test)]
    pub fn blocking_process_new_block(
        &self,
        block: core::BlockView,
        _peer_id: PeerIndex,
    ) -> Result<bool, ckb_error::Error> {
        let block_hash = block.hash();
        let status = self.shared.active_chain().get_block_status(&block_hash);
        // NOTE: Filtering `BLOCK_STORED` but not `BLOCK_RECEIVED`, is for avoiding
        // stopping synchronization even when orphan_pool maintains dirty items by bugs.
        if status.contains(BlockStatus::BLOCK_STORED) {
            error!("block {} already stored", block_hash);
            Ok(false)
        } else if status.contains(BlockStatus::HEADER_VALID) {
            self.chain.blocking_process_block(Arc::new(block))
        } else {
            debug!(
                "Synchronizer process_new_block unexpected status {:?} {}",
                status, block_hash,
            );
            // TODO while error should we return?
            Ok(false)
        }
    }

    /// Get blocks to fetch
    pub fn get_blocks_to_fetch(
        &self,
        peer: PeerIndex,
        ibd: IBDState,
    ) -> Option<Vec<Vec<packed::Byte32>>> {
        BlockFetcher::new(Arc::clone(&self.shared), peer, ibd).fetch(BlockNumber::MAX)
    }

    pub(crate) fn on_connected(&self, nc: &dyn CKBProtocolContext, peer: PeerIndex) {
        let pid = SupportProtocols::Sync.protocol_id();
        let (is_outbound, is_whitelist, is_2023edition) = nc
            .get_peer(peer)
            .map(|peer| {
                (
                    peer.is_outbound(),
                    peer.is_whitelist,
                    peer.protocols.get(&pid).map(|v| v == "3").unwrap_or(false),
                )
            })
            .unwrap_or((false, false, false));

        self.peers()
            .sync_connected(peer, is_outbound, is_whitelist, is_2023edition);
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
                    active_chain.send_getheaders_to_peer(
                        nc,
                        *peer,
                        better_tip_header.number_and_hash(),
                    );
                }
            }

            // On ibd, node should only have one peer to sync headers, and it's state can control by
            // headers_sync_controller.
            //
            // The header sync of other nodes does not matter in the ibd phase, and parallel synchronization
            // can be enabled by unknown list, so there is no need to repeatedly download headers with
            // multiple nodes at the same time.
            if active_chain.is_initial_block_download() {
                continue;
            }
            if state.peer_flags.is_outbound {
                let best_known_header = state.best_known_header.as_ref();
                let (tip_header, local_total_difficulty) = {
                    (
                        active_chain.tip_header().to_owned(),
                        active_chain.total_difficulty().to_owned(),
                    )
                };
                if best_known_header
                    .map(|header_index| header_index.total_difficulty().clone())
                    .unwrap_or_default()
                    >= local_total_difficulty
                {
                    if state.chain_sync.timeout != 0 {
                        state.chain_sync.timeout = 0;
                        state.chain_sync.work_header = None;
                        state.chain_sync.total_difficulty = None;
                        state.chain_sync.sent_getheaders = false;
                    }
                } else if state.chain_sync.timeout == 0
                    || (best_known_header.is_some()
                        && best_known_header
                            .map(|header_index| header_index.total_difficulty().clone())
                            >= state.chain_sync.total_difficulty)
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
                            state
                                .chain_sync
                                .work_header
                                .as_ref()
                                .expect("work_header be assigned")
                                .into(),
                        );
                    }
                }
            }
        }
        for peer in eviction {
            info!("Timeout eviction peer={}", peer);
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

            debug!("Start sync peer={}", peer);
            active_chain.send_getheaders_to_peer(nc, peer, tip.number_and_hash());
        }
    }

    fn get_peers_to_fetch(
        &self,
        ibd: IBDState,
        disconnect_list: &HashSet<PeerIndex>,
    ) -> Vec<PeerIndex> {
        trace!("Poll find_blocks_to_fetch selecting peers");
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
        if self.chain.is_verifying_unverified_blocks_on_startup() {
            trace!(
                "skip find_blocks_to_fetch, ckb_chain is verifying unverified blocks on startup"
            );
            return;
        }

        if ckb_stop_handler::has_received_stop_signal() {
            info!("received stop signal, stop find_blocks_to_fetch");
            return;
        }

        let unverified_tip = self.shared.active_chain().unverified_tip_number();

        let disconnect_list = {
            let mut list = self
                .shared()
                .state()
                .write_inflight_blocks()
                .prune(unverified_tip);
            if let IBDState::In = ibd {
                // best known < tip and in IBD state, and unknown list is empty,
                // these node can be disconnect
                list.extend(
                    self.shared
                        .state()
                        .peers()
                        .get_best_known_less_than_tip_and_unknown_empty(unverified_tip),
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
                        let _ignore = sender.try_send(FetchCMD {
                            peers,
                            ibd_state: ibd,
                        });
                    }
                }
                None => {
                    let p2p_control = raw.clone();
                    let (sender, recv) = channel::bounded(2);
                    let peers = self.get_peers_to_fetch(ibd, &disconnect_list);
                    sender
                        .send(FetchCMD {
                            peers,
                            ibd_state: ibd,
                        })
                        .unwrap();
                    self.fetch_channel = Some(sender);
                    let thread = ::std::thread::Builder::new();
                    let number = self.shared.state().shared_best_header_ref().number();
                    const THREAD_NAME: &str = "BlockDownload";
                    let sync_shared: Arc<SyncShared> = Arc::to_owned(self.shared());
                    let blockdownload_jh = thread
                        .name(THREAD_NAME.into())
                        .spawn(move || {
                            let stop_signal = new_crossbeam_exit_rx();
                            BlockFetchCMD {
                                sync_shared,
                                p2p_control,
                                recv,
                                number,
                                can_start: CanStart::MinWorkNotReach,
                                start_timestamp: unix_time_as_millis(),
                            }
                            .run(stop_signal);
                        })
                        .expect("download thread can't start");
                    register_thread(THREAD_NAME, blockdownload_jh);
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
                    if reader.has_extra_fields() || reader.block().count_extra_fields() > 1 {
                        info!(
                            "A malformed message from peer {}: \
                             excessive fields detected in SendBlock",
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
                                "A malformed message from peer {}: \
                                 excessive fields",
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
                info!("A malformed message from peer {}", peer_index);
                nc.ban_peer(
                    peer_index,
                    BAD_MESSAGE_BAN_TIME,
                    String::from("send us a malformed message"),
                );
                return;
            }
        };

        debug!("Received msg {} from {}", msg.item_name(), peer_index);
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
        tokio::task::block_in_place(|| self.process(nc, peer_index, msg));
        debug!(
            "Process message={}, peer={}, cost={:?}",
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
        let sync_state = self.shared().state();
        sync_state.disconnected(peer_index);
        info!("SyncProtocol.disconnected peer={}", peer_index);
    }

    async fn notify(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>, token: u64) {
        if !self.peers().state.is_empty() {
            let start_time = Instant::now();
            trace!("Start notify token={}", token);
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
                            trace!("Ibd block fetch token removal failed");
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
                "Finished notify token={} cost={:?}",
                token,
                Instant::now().saturating_duration_since(start_time)
            );
        } else if token == NO_PEER_CHECK_TOKEN {
            debug!("No peers connected");
        }
    }
}
