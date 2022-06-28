use ckb_merkle_mountain_range::{leaf_index_to_mmr_size, leaf_index_to_pos};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*, utilities::merkle_mountain_range::ChainRootMMR};

use crate::{prelude::LightClientProtocolReply, LightClientProtocol, Status, StatusCode};

pub(crate) struct GetBlockProofProcess<'a> {
    message: packed::GetBlockProofReader<'a>,
    protocol: &'a LightClientProtocol,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

impl<'a> GetBlockProofProcess<'a> {
    pub(crate) fn new(
        message: packed::GetBlockProofReader<'a>,
        protocol: &'a LightClientProtocol,
        peer: PeerIndex,
        nc: &'a dyn CKBProtocolContext,
    ) -> Self {
        Self {
            message,
            protocol,
            peer,
            nc,
        }
    }

    pub(crate) fn execute(self) -> Status {
        let active_chain = self.protocol.shared.active_chain();
        let snapshot = self.protocol.shared.shared().snapshot();

        let block_hashes: Vec<_> = self
            .message
            .block_hashes()
            .iter()
            .map(|hash| hash.to_entity())
            .collect();
        let tip_hash = self.message.tip_hash().to_entity();

        let tip_header = if let Some(header) = active_chain.get_block_header(&tip_hash) {
            header
        } else {
            // The tip_hash is not on the chain
            let message = packed::LightClientMessage::new_builder()
                .set(packed::SendBlockProof::default())
                .build();
            self.nc.reply(self.peer, &message);
            return Status::ok();
        };
        let block_headers: Vec<_> = block_hashes
            .iter()
            .filter_map(|hash| active_chain.get_block_header(hash))
            .map(|header| header.number())
            .filter_map(|number| active_chain.get_ancestor(&tip_hash, number))
            .collect();

        let positions: Vec<_> = block_headers
            .iter()
            .map(|header| leaf_index_to_pos(header.number()))
            .collect();
        let mmr_size = leaf_index_to_mmr_size(tip_header.number() - 1);
        let mmr = ChainRootMMR::new(mmr_size, &**snapshot);
        let root = match mmr.get_root() {
            Ok(root) => root,
            Err(err) => {
                let errmsg = format!("failed to generate a root since {:?}", err);
                return StatusCode::InternalError.with_context(errmsg);
            }
        };
        let proof = match mmr.gen_proof(positions) {
            Ok(proof) => proof,
            Err(err) => {
                let errmsg = format!("failed to generate a proof since {:?}", err);
                return StatusCode::InternalError.with_context(errmsg);
            }
        };
        let content = packed::SendBlockProof::new_builder()
            .root(root)
            .proof(proof.pack())
            .headers(block_headers.into_iter().map(|view| view.data()).pack())
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();
        self.nc.reply(self.peer, &message);

        Status::ok()
    }
}
