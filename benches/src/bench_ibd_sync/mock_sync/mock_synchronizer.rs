use crate::mock_sync::headers_process::HeadersProcess;
use crate::{
    mock_sync::get_blocks_process::GetBlocksProcess,
    mock_sync::get_headers_process::GetHeadersProcess,
};
use ckb_constant::sync::BAD_MESSAGE_BAN_TIME;
use ckb_logger::{debug, error, info, trace, warn};
use ckb_network::{
    async_trait, bytes::Bytes, tokio, CKBProtocolContext, CKBProtocolHandler, PeerIndex,
};

use crate::mock_sync::in_ibd_process::InIBDProcess;
use ckb_sync::{
    types::{HeadersSyncController, Peers, SyncShared},
    Status, StatusCode,
};
use ckb_types::{core, packed, prelude::*};
use faketime::unix_time_as_millis;
use std::sync::atomic::Ordering;
use std::time::Instant;
use std::{sync::Arc, time::Duration};

pub const SEND_GET_HEADERS_TOKEN: u64 = 0;
pub const TIMEOUT_EVICTION_TOKEN: u64 = 3;
pub const NO_PEER_CHECK_TOKEN: u64 = 255;
const SYNC_NOTIFY_INTERVAL: Duration = Duration::from_secs(1);

/// Sync protocol handle
#[derive(Clone)]
pub struct MockSynchronizer {
    /// Sync shared state
    pub shared: Arc<SyncShared>,
}

impl MockSynchronizer {
    /// Init sync protocol handle
    ///
    /// This is a runtime sync protocol shared state, and any relay messages will be processed and forwarded by it
    pub fn new(shared: Arc<SyncShared>) -> MockSynchronizer {
        MockSynchronizer { shared }
    }

    /// Get shared state
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

    pub(crate) fn on_connected(&self, nc: &dyn CKBProtocolContext, peer: PeerIndex) {
        let (is_outbound, is_whitelist) = nc
            .get_peer(peer)
            .map(|peer| (peer.is_outbound(), peer.is_whitelist))
            .unwrap_or((false, false));

        self.peers().sync_connected(peer, is_outbound, is_whitelist);
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
}

#[async_trait]
impl CKBProtocolHandler for MockSynchronizer {
    async fn init(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>) {
        nc.set_notify(SYNC_NOTIFY_INTERVAL, SEND_GET_HEADERS_TOKEN)
            .await
            .expect("set_notify at init is ok");
        nc.set_notify(SYNC_NOTIFY_INTERVAL, TIMEOUT_EVICTION_TOKEN)
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
}
