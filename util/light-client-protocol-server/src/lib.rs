//! Server-side implementation for CKB light client protocol.
//!
//! TODO(light-client) More documentation.

use std::sync::Arc;

use ckb_logger::{debug, error, info, trace, warn};
use ckb_network::{async_trait, bytes::Bytes, CKBProtocolContext, CKBProtocolHandler, PeerIndex};
use ckb_sync::SyncShared;
use ckb_types::{core, packed, prelude::*};

use crate::prelude::*;

mod components;
mod constant;
mod prelude;
mod status;

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
        match message {
            packed::LightClientMessageUnionReader::GetLastState(reader) => {
                components::GetLastStateProcess::new(reader, self, peer_index, nc).execute()
            }
            packed::LightClientMessageUnionReader::GetLastStateProof(reader) => {
                components::GetLastStateProofProcess::new(reader, self, peer_index, nc).execute()
            }
            packed::LightClientMessageUnionReader::GetBlocksProof(reader) => {
                components::GetBlocksProofProcess::new(reader, self, peer_index, nc).execute()
            }
            packed::LightClientMessageUnionReader::GetTransactionsProof(reader) => {
                components::GetTransactionsProofProcess::new(reader, self, peer_index, nc).execute()
            }
            _ => StatusCode::UnexpectedProtocolMessage.into(),
        }
    }

    pub(crate) fn get_verifiable_tip_header(&self) -> Result<packed::VerifiableHeader, String> {
        let active_chain = self.shared.active_chain();

        let tip_hash = active_chain.tip_hash();
        let tip_block = active_chain
            .get_block(&tip_hash)
            .expect("checked: tip block should be existed");
        let parent_chain_root = {
            let snapshot = self.shared.shared().snapshot();
            let mmr = snapshot.chain_root_mmr(tip_block.number() - 1);
            match mmr.get_root() {
                Ok(root) => root,
                Err(err) => {
                    let errmsg = format!("failed to generate a root since {:?}", err);
                    return Err(errmsg);
                }
            }
        };

        let tip_header = packed::VerifiableHeader::new_builder()
            .header(tip_block.header().data())
            .uncles_hash(tip_block.calc_uncles_hash())
            .extension(Pack::pack(&tip_block.extension()))
            .parent_chain_root(parent_chain_root)
            .build();

        Ok(tip_header)
    }

    pub(crate) fn reply_tip_state<T>(&self, peer: PeerIndex, nc: &dyn CKBProtocolContext) -> Status
    where
        T: Entity,
        <T as Entity>::Builder: ProverMessageBuilder,
        <<T as Entity>::Builder as Builder>::Entity: Into<packed::LightClientMessageUnion>,
    {
        let tip_header = match self.get_verifiable_tip_header() {
            Ok(tip_state) => tip_state,
            Err(errmsg) => {
                return StatusCode::InternalError.with_context(errmsg);
            }
        };
        let content = T::new_builder().set_last_header(tip_header).build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();
        nc.reply(peer, &message);
        Status::ok()
    }

    pub(crate) fn reply_proof<T>(
        &self,
        peer: PeerIndex,
        nc: &dyn CKBProtocolContext,
        last_block: &core::BlockView,
        items_positions: Vec<u64>,
        items: <<T as Entity>::Builder as ProverMessageBuilder>::Items,
    ) -> Status
    where
        T: Entity,
        <T as Entity>::Builder: ProverMessageBuilder,
        <<T as Entity>::Builder as Builder>::Entity: Into<packed::LightClientMessageUnion>,
    {
        let (parent_chain_root, proof) = {
            let snapshot = self.shared.shared().snapshot();
            let mmr = snapshot.chain_root_mmr(last_block.number() - 1);
            let parent_chain_root = match mmr.get_root() {
                Ok(root) => root,
                Err(err) => {
                    let errmsg = format!("failed to generate a root since {:?}", err);
                    return StatusCode::InternalError.with_context(errmsg);
                }
            };
            let proof = match mmr.gen_proof(items_positions) {
                Ok(proof) => proof.proof_items().to_owned(),
                Err(err) => {
                    let errmsg = format!("failed to generate a proof since {:?}", err);
                    return StatusCode::InternalError.with_context(errmsg);
                }
            };
            (parent_chain_root, proof)
        };
        let verifiable_last_header = packed::VerifiableHeader::new_builder()
            .header(last_block.data().header())
            .uncles_hash(last_block.calc_uncles_hash())
            .extension(Pack::pack(&last_block.extension()))
            .parent_chain_root(parent_chain_root)
            .build();
        let content = T::new_builder()
            .set_last_header(verifiable_last_header)
            .set_proof(proof.pack())
            .set_items(items)
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();
        nc.reply(peer, &message);
        Status::ok()
    }
}
