//! Server-side implementation for CKB light client protocol.
//!
//! TODO(light-client) More documentation.

use std::sync::Arc;

use ckb_logger::{debug, error, info, trace, warn};
use ckb_network::{async_trait, bytes::Bytes, CKBProtocolContext, CKBProtocolHandler, PeerIndex};
use ckb_sync::SyncShared;
use ckb_types::{packed, prelude::*};

mod components;
mod constant;
mod prelude;
mod status;

use prelude::LightClientProtocolReply;
pub use status::{Status, StatusCode};

/// Light client protocol handler.
pub struct LightClientProtocol {
    /// Sync shared state.
    pub shared: Arc<SyncShared>,
}

impl LightClientProtocol {
    /// Create a new light client protocol handler.
    pub fn new(shared: Arc<SyncShared>) -> Self {
        Self { shared }
    }
}

#[async_trait]
impl CKBProtocolHandler for LightClientProtocol {
    async fn init(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>) {}

    async fn connected(
        &mut self,
        _nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        version: &str,
    ) {
        info!("LightClient({}).connected peer={}", version, peer);
    }

    async fn disconnected(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>, peer: PeerIndex) {
        info!("LightClient.disconnected peer={}", peer);
    }

    async fn received(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        data: Bytes,
    ) {
        trace!("LightClient.received peer={}", peer);

        let msg = match packed::LightClientMessageReader::from_slice(&data) {
            Ok(msg) => msg.to_enum(),
            _ => {
                warn!(
                    "LightClient.received a malformed message from Peer({})",
                    peer
                );
                nc.ban_peer(
                    peer,
                    constant::BAD_MESSAGE_BAN_TIME,
                    String::from("send us a malformed message"),
                );
                return;
            }
        };

        let item_name = msg.item_name();
        let status = self.try_process(nc.as_ref(), peer, msg);
        if let Some(ban_time) = status.should_ban() {
            error!(
                "process {} from {}, ban {:?} since result is {}",
                item_name, peer, ban_time, status
            );
            nc.ban_peer(peer, ban_time, status.to_string());
        } else if status.should_warn() {
            warn!("process {} from {}, result is {}", item_name, peer, status);
        } else if !status.is_ok() {
            debug!("process {} from {}, result is {}", item_name, peer, status);
        }
    }
}

impl LightClientProtocol {
    fn try_process(
        &mut self,
        nc: &dyn CKBProtocolContext,
        peer_index: PeerIndex,
        message: packed::LightClientMessageUnionReader<'_>,
    ) -> Status {
        if !matches!(
            message,
            packed::LightClientMessageUnionReader::GetLastState(_)
        ) && !self.peer_is_lightclient(nc, peer_index)
        {
            return StatusCode::UnexpectedProtocolMessage.into();
        }

        match message {
            packed::LightClientMessageUnionReader::GetLastState(reader) => {
                components::GetLastStateProcess::new(reader, self, peer_index, nc).execute()
            }
            packed::LightClientMessageUnionReader::GetBlockSamples(reader) => {
                components::GetBlockSamplesProcess::new(reader, self, peer_index, nc).execute()
            }
            packed::LightClientMessageUnionReader::GetBlockProof(reader) => {
                components::GetBlockProofProcess::new(reader, self, peer_index, nc).execute()
            }
            packed::LightClientMessageUnionReader::GetTransactions(reader) => {
                components::GetTransactionsProcess::new(reader, self, peer_index, nc).execute()
            }
            _ => StatusCode::UnexpectedProtocolMessage.into(),
        }
    }

    fn peer_is_lightclient(&self, nc: &dyn CKBProtocolContext, peer: PeerIndex) -> bool {
        nc.get_peer(peer)
            .map(|peer| peer.is_lightclient)
            .unwrap_or(false)
    }

    pub(crate) fn send_last_state(&self, nc: &dyn CKBProtocolContext, peer: PeerIndex) -> Status {
        let active_chain = self.shared.active_chain();

        let tip_hash = active_chain.tip_hash();
        let tip_block = active_chain
            .get_block(&tip_hash)
            .expect("checked: tip block should be existed");
        let tip_header = packed::VerifiableHeader::new_builder()
            .header(tip_block.header().data())
            .uncles_hash(tip_block.calc_uncles_hash())
            .extension(Pack::pack(&tip_block.extension()))
            .build();
        let total_difficulty = active_chain
            .get_block_ext(&tip_hash)
            .map(|block_ext| block_ext.total_difficulty)
            .expect("checked: tip block should have block ext");

        let content = packed::SendLastState::new_builder()
            .tip_header(tip_header)
            .total_difficulty(total_difficulty.pack())
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();

        nc.reply(peer, &message)
    }
}
