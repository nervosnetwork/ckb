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

    fn send_empty_proof(&self) {
        let message = packed::LightClientMessage::new_builder()
            .set(packed::SendBlockProof::default())
            .build();
        self.nc.reply(self.peer, &message);
    }

    pub(crate) fn execute(self) -> Status {
        let active_chain = self.protocol.shared.active_chain();
        let snapshot = self.protocol.shared.shared().snapshot();

        let block_hash = self.message.block_hash().to_entity();
        let tip_hash = self.message.tip_hash().to_entity();

        let block_header = if let Some(header) = active_chain.get_block_header(&block_hash) {
            header
        } else {
            // The block is not on the chain
            self.send_empty_proof();
            return Status::ok();
        };
        let tip_header =
            if let Some(header) = active_chain.get_ancestor(&tip_hash, block_header.number()) {
                header
            } else {
                // The tip_hash is not on the chain or block_hash is not ancestor of tip_hash
                self.send_empty_proof();
                return Status::ok();
            };
        let uncles_hash = if let Some(block) = active_chain.get_block(&block_hash) {
            block.calc_uncles_hash()
        } else {
            let errmsg = format!(
                "failed to find block for header#{} (hash: {:#x})",
                block_header.number(),
                block_hash
            );
            return StatusCode::InternalError.with_context(errmsg);
        };

        let positions = vec![leaf_index_to_pos(block_header.number())];
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
        let content = {
            let header_with_chain_root = packed::HeaderWithChainRoot::new_builder()
                .header(block_header.data())
                .uncles_hash(uncles_hash)
                .chain_root(root)
                .build();
            let single_block_proof = packed::SingleBlockProof::new_builder()
                .proof(proof.pack())
                .header(header_with_chain_root)
                .build();
            packed::SendBlockProof::new_builder()
                .proof(Some(single_block_proof).pack())
                .build()
        };
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();
        self.nc.reply(self.peer, &message);

        Status::ok()
    }
}
