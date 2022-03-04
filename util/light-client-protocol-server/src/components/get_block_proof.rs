use ckb_merkle_mountain_range::{leaf_index_to_mmr_size, leaf_index_to_pos};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_shared::Snapshot;
use ckb_types::{
    core::BlockNumber, packed, prelude::*, utilities::merkle_mountain_range::ChainRootMMR,
};

use crate::{prelude::*, LightClientProtocol, Status, StatusCode};

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
        let consensus = self.protocol.shared.consensus();
        let mmr_activated_number = consensus.mmr_activated_number();

        let last_block_hash = self.message.last_hash().to_entity();
        let last_block = if let Some(block) = active_chain.get_block(&last_block_hash) {
            block
        } else {
            let error_message = format!(
                "the last block ({:#x}) sent from the client is not existed",
                last_block_hash
            );
            return StatusCode::InvalidLastBlock.with_context(error_message);
        };
        let last_block_number = last_block.number();

        let (positions, headers) = {
            let mut headers: Vec<packed::Header> = Vec::new();
            let mut positions: Vec<u64> = Vec::new();
            for block_number in self.message.numbers().iter() {
                let block_number: BlockNumber = block_number.unpack();

                if block_number > last_block_number {
                    let error_message = format!(
                        "the unconfirmed block ({}) is not before the last block ({})",
                        block_number, last_block_number
                    );
                    return StatusCode::InvalidUnconfirmedBlock.with_context(error_message);
                } else if block_number < mmr_activated_number {
                    let error_message = format!(
                        "the unconfirmed block ({}) is before the MMR activated block ({})",
                        block_number, mmr_activated_number
                    );
                    return StatusCode::InvalidUnconfirmedBlock.with_context(error_message);
                } else if let Some(ancestor_header) =
                    active_chain.get_ancestor(&last_block_hash, block_number)
                {
                    let index = block_number - mmr_activated_number;
                    let position = leaf_index_to_pos(index);
                    positions.push(position);
                    headers.push(ancestor_header.data());
                } else {
                    let error_message =
                        format!("failed to find ancestor header ({})", block_number);
                    return StatusCode::InternalError.with_context(error_message);
                }
            }
            (positions, headers)
        };

        let (root, proof) = {
            let mmr_size = leaf_index_to_mmr_size(last_block_number - mmr_activated_number);
            let snapshot_ref: &Snapshot = &snapshot;
            let mmr = ChainRootMMR::new(mmr_size, snapshot_ref);
            let root = match mmr.get_root() {
                Ok(root) => root,
                Err(err) => {
                    let error_message = format!("failed to generate a root since {:?}", err);
                    return StatusCode::InternalError.with_context(error_message);
                }
            };
            let proof = match mmr.gen_proof(positions) {
                Ok(proof) => proof,
                Err(err) => {
                    let error_message = format!("failed to generate a proof since {:?}", err);
                    return StatusCode::InternalError.with_context(error_message);
                }
            };
            (root, proof)
        };

        let content = packed::SendBlockProof::new_builder()
            .root(root)
            .proof(proof.pack())
            .headers(headers.pack())
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();

        self.nc.reply(self.peer, &message)
    }
}
