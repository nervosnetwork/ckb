use std::collections::HashSet;

use ckb_merkle_mountain_range::leaf_index_to_pos;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};

use crate::{LightClientProtocol, Status, StatusCode};

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

        let last_hash = self.message.last_hash().to_entity();
        let last_block = if let Some(block) = active_chain.get_block(&last_hash) {
            block
        } else {
            return self
                .protocol
                .reply_tip_state::<packed::SendBlockProof>(self.peer, self.nc);
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

        self.protocol.reply_proof::<packed::SendBlockProof>(
            self.peer,
            self.nc,
            &last_block,
            positions,
            proved_items,
        )
    }
}
