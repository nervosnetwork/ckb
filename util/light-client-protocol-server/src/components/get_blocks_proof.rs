use std::collections::HashSet;

use ckb_merkle_mountain_range::leaf_index_to_pos;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_store::ChainStore;
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
        if self.message.block_hashes().is_empty() {
            return StatusCode::MalformedProtocolMessage.with_context("no block");
        }

        if self.message.block_hashes().len() > constant::GET_BLOCKS_PROOF_LIMIT {
            return StatusCode::MalformedProtocolMessage.with_context("too many blocks");
        }

        let snapshot = self.protocol.shared.snapshot();

        let last_hash = self.message.last_hash().to_entity();
        let last_block = if let Some(block) = snapshot.get_block(&last_hash) {
            block
        } else {
            return self
                .protocol
                .reply_tip_state::<packed::SendBlocksProof>(self.peer, self.nc);
        };

        let block_hashes: Vec<_> = self
            .message
            .block_hashes()
            .to_entity()
            .into_iter()
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

        let (positions, block_headers, missing_blocks) = block_hashes
            .into_iter()
            .map(|block_hash| {
                snapshot
                    .get_block_header(&block_hash)
                    .map(|header| header.number())
                    .filter(|number| *number != last_block.number())
                    .and_then(|number| snapshot.get_ancestor(&last_hash, number))
                    .filter(|header| header.hash() == block_hash)
                    .ok_or(block_hash)
            })
            .fold(
                (Vec::new(), Vec::new(), Vec::new()),
                |(mut positions, mut block_headers, mut missing_blocks), result| {
                    match result {
                        Ok(header) => {
                            positions.push(leaf_index_to_pos(header.number()));
                            block_headers.push(header);
                        }
                        Err(block_hash) => {
                            missing_blocks.push(block_hash);
                        }
                    }
                    (positions, block_headers, missing_blocks)
                },
            );

        let proved_items = block_headers.into_iter().map(|view| view.data()).pack();
        let missing_items = missing_blocks.pack();

        self.protocol.reply_proof::<packed::SendBlocksProof>(
            self.peer,
            self.nc,
            &last_block,
            positions,
            proved_items,
            missing_items,
        )
    }
}
