use super::block_fetch_cmd::{BlockFetchCMD, CanStart, FetchCMD};
use super::block_fetcher::BlockFetcher;
use super::{BlockProcess, GetBlocksProcess, GetHeadersProcess, HeadersProcess, InIBDProcess};
use crate::types::{HeadersSyncController, IBDState, Peers, SyncShared, post_sync_process};
use crate::utils::{MetricDirection, async_send_message_to, metric_ckb_message_bytes};
use crate::{Status, StatusCode};
use ckb_shared::block_status::BlockStatus;

use ckb_chain::{ChainController, RemoteBlock};
use ckb_channel as channel;
use ckb_constant::sync::{
    BAD_MESSAGE_BAN_TIME, CHAIN_SYNC_TIMEOUT, EVICTION_HEADERS_RESPONSE_TIME,
    INIT_BLOCKS_IN_TRANSIT_PER_PEER,
};
use ckb_logger::{debug, error, info, trace};
use ckb_metrics::HistogramTimer;
use ckb_network::{
    CKBProtocolContext, CKBProtocolHandler, PeerIndex, SupportProtocols, async_trait, bytes::Bytes,
    tokio,
};
use ckb_shared::types::HeaderIndexView;
use ckb_stop_handler::{new_crossbeam_exit_rx, register_thread};
use ckb_systemtime::unix_time_as_millis;

#[cfg(test)]
use ckb_types::core;
use ckb_types::{
    core::BlockNumber,
    packed::{self},
    prelude::*,
};
use std::{
    collections::HashSet,
    sync::{Arc, atomic::Ordering},
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

    async fn try_process(
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
                tokio::task::block_in_place(|| {
                    GetHeadersProcess::new(reader, self, peer, &nc).execute()
                })
            }
            packed::SyncMessageUnionReader::SendHeaders(reader) => {
                tokio::task::block_in_place(|| {
                    HeadersProcess::new(reader, self, peer, &nc).execute()
                })
            }
            packed::SyncMessageUnionReader::GetBlocks(reader) => {
                tokio::task::block_in_place(|| {
                    GetBlocksProcess::new(reader, self, peer, &nc).execute()
                })
            }
            packed::SyncMessageUnionReader::SendBlock(reader) => {
                if reader.check_data() {
                    BlockProcess::new(reader, self, peer, nc).execute()
                } else {
                    StatusCode::ProtocolMessageIsMalformed.with_context("SendBlock is invalid")
                }
            }
            packed::SyncMessageUnionReader::InIBD(_) => {
                InIBDProcess::new(self, peer, &nc).execute().await
            }
        }
    }

    async fn process(
        &self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        message: packed::SyncMessageUnionReader<'_>,
    ) {
        let item_name = message.item_name();
        let item_bytes = message.as_slice().len() as u64;
        let status = self.try_process(Arc::clone(&nc), peer, message).await;

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

    /// Process new block in blocking way
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
    pub fn eviction(&self, nc: &Arc<dyn CKBProtocolContext + Sync>) {
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

    fn start_sync_headers(&self, nc: &Arc<dyn CKBProtocolContext + Sync>) {
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
                    if ibd && x != 0 { None } else { Some(x + 1) }
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

    fn find_blocks_to_fetch(&mut self, nc: &Arc<dyn CKBProtocolContext + Sync>, ibd: IBDState) {
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
            let nc = Arc::clone(nc);
            let peer = *peer;
            self.shared.shared().async_handle().spawn(async move {
                let _status = nc.async_disconnect(peer, "sync disconnect").await;
            });
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
                    if let Some(fetch) =
                        tokio::task::block_in_place(|| self.get_blocks_to_fetch(peer, ibd))
                    {
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
        nc: &Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
    ) {
        let content = packed::GetBlocks::new_builder()
            .block_hashes(v_fetch.clone())
            .build();
        let message = packed::SyncMessage::new_builder().set(content).build();

        debug!("send_getblocks len={:?} to peer={}", v_fetch.len(), peer);
        let nc = Arc::clone(nc);
        self.shared.shared().async_handle().spawn(async move {
            let _status = async_send_message_to(&nc, peer, &message).await;
        });
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
        self.process(nc, peer_index, msg).await;
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
                    self.start_sync_headers(&nc);
                }
                IBD_BLOCK_FETCH_TOKEN => {
                    if self.shared.active_chain().is_initial_block_download() {
                        self.find_blocks_to_fetch(&nc, IBDState::In);
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
                        self.find_blocks_to_fetch(&nc, IBDState::Out);
                    }
                }
                TIMEOUT_EVICTION_TOKEN => {
                    self.eviction(&nc);
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
