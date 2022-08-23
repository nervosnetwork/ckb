use std::collections::HashSet;

use ckb_merkle_mountain_range::leaf_index_to_pos;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};

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

    fn reply_only_the_tip_state(&self) -> Status {
        let tip_header = match self.protocol.get_verifiable_tip_header() {
            Ok(tip_state) => tip_state,
            Err(errmsg) => {
                return StatusCode::InternalError.with_context(errmsg);
            }
        };
        let content = packed::SendBlockProof::new_builder()
            .tip_header(tip_header)
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();
        self.nc.reply(self.peer, &message);
        Status::ok()
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

        let mut uniq = HashSet::new();
        if !block_hashes
            .iter()
            .chain([tip_hash.clone()].iter())
            .all(|hash| uniq.insert(hash))
        {
            return StatusCode::MalformedProtocolMessage
                .with_context("block_hashes and tip_hash should be uniq");
        }

        let tip_block = if let Some(block) = active_chain.get_block(&tip_hash) {
            block
        } else {
            return self.reply_only_the_tip_state();
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
        let mmr = snapshot.chain_root_mmr(tip_block.number() - 1);
        let parent_chain_root = match mmr.get_root() {
            Ok(root) => root,
            Err(err) => {
                let errmsg = format!("failed to generate a root since {:?}", err);
                return StatusCode::InternalError.with_context(errmsg);
            }
        };
        let proof = match mmr.gen_proof(positions) {
            Ok(proof) => proof.proof_items().to_owned(),
            Err(err) => {
                let errmsg = format!("failed to generate a proof since {:?}", err);
                return StatusCode::InternalError.with_context(errmsg);
            }
        };

        let verifiable_tip_header = packed::VerifiableHeader::new_builder()
            .header(tip_block.data().header())
            .uncles_hash(tip_block.calc_uncles_hash())
            .extension(Pack::pack(&tip_block.extension()))
            .parent_chain_root(parent_chain_root)
            .build();
        let content = packed::SendBlockProof::new_builder()
            .proof(proof.pack())
            .tip_header(verifiable_tip_header)
            .headers(block_headers.into_iter().map(|view| view.data()).pack())
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();
        self.nc.reply(self.peer, &message);

        Status::ok()
    }
}
