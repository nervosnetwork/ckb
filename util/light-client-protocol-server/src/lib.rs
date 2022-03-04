//! Server-side implementation for CKB light client protocol.
//!
//! TODO(light-client) More documentation.

use std::sync::Arc;

use ckb_logger::{debug, error, info, trace, warn};
use ckb_network::{bytes::Bytes, CKBProtocolContext, CKBProtocolHandler, PeerIndex};
use ckb_sync::SyncShared;
use ckb_types::{packed, prelude::*};

mod components;
pub mod constant;
mod prelude;
mod status;

pub use status::{Status, StatusCode};

pub struct LightClientProtocol {
    pub shared: Arc<SyncShared>,
}

impl LightClientProtocol {
    pub fn new(shared: Arc<SyncShared>) -> Self {
        Self { shared }
    }
}

impl CKBProtocolHandler for LightClientProtocol {
    fn init(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>) {}

    fn connected(
        &mut self,
        _nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
        version: &str,
    ) {
        info!("LightClient({}).connected peer={}", version, peer);
    }

    fn disconnected(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>, peer: PeerIndex) {
        info!("LightClient.disconnected peer={}", peer);
    }

    fn received(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>, peer: PeerIndex, data: Bytes) {
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
        peer: PeerIndex,
        message: packed::LightClientMessageUnionReader<'_>,
    ) -> Status {
        match message {
            packed::LightClientMessageUnionReader::GetChainInfo(reader) => {
                components::GetChainInfoProcess::new(reader, self, peer, nc).execute()
            }
            packed::LightClientMessageUnionReader::GetLastHeader(reader) => {
                components::GetLastHeaderProcess::new(reader, self, peer, nc).execute()
            }
            packed::LightClientMessageUnionReader::GetBlockProof(reader) => {
                components::GetBlockProofProcess::new(reader, self, peer, nc).execute()
            }
            _ => StatusCode::UnexpectedProtocolMessage.into(),
        }
    }
}
