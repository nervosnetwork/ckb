use std::collections::HashSet;

use ckb_merkle_mountain_range::leaf_index_to_pos;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};

use crate::{constant, LightClientProtocol, Status, StatusCode};

pub(crate) struct GetBlocksProofProcess<'a> {
    message: packed::GetBlocksProofReader<'a>,
    protocol: &'a LightClientProtocol,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

impl<'a> GetBlocksProofProcess<'a> {
    pub(crate) fn new(
        message: packed::GetBlocksProofReader<'a>,
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
        if self.message.block_hashes().len() > constant::GET_BLOCKS_PROOF_LIMIT {
            return StatusCode::MalformedProtocolMessage.with_context("too many blocks");
        }

        let active_chain = self.protocol.shared.active_chain();

        let last_hash = self.message.last_hash().to_entity();
        let last_block = if let Some(block) = active_chain.get_block(&last_hash) {
            block
        } else {
            return self
                .protocol
                .reply_tip_state::<packed::SendBlocksProof>(self.peer, self.nc);
        };

        let block_hashes: Vec<_> = self
            .message
            .block_hashes()
            .iter()
            .map(|hash| hash.to_entity())
            .collect();

        let mut uniq = HashSet::new();
        if !block_hashes
            .iter()
            .chain([last_hash.clone()].iter())
            .all(|hash| uniq.insert(hash))
        {
            return StatusCode::MalformedProtocolMessage
                .with_context("duplicate block hash exists");
        }

        let block_headers: Vec<_> = block_hashes
            .iter()
            .filter_map(|hash| active_chain.get_block_header(hash))
            .map(|header| header.number())
            .filter_map(|number| active_chain.get_ancestor(&last_hash, number))
            .collect();

        let positions: Vec<_> = block_headers
            .iter()
            .map(|header| leaf_index_to_pos(header.number()))
            .collect();

        let proved_items = block_headers.into_iter().map(|view| view.data()).pack();

        self.protocol.reply_proof::<packed::SendBlocksProof>(
            self.peer,
            self.nc,
            &last_block,
            positions,
            proved_items,
        )
    }
}
