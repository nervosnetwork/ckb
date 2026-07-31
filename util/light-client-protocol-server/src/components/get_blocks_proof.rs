use std::collections::HashSet;

use ckb_merkle_mountain_range::leaf_index_to_pos;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_store::ChainStore;
use ckb_types::{packed, prelude::*};
use std::sync::Arc;

use crate::{LightClientProtocol, Status, StatusCode, constant};

pub(crate) struct GetBlocksProofProcess<'a> {
    message: packed::GetBlocksProofReader<'a>,
    protocol: &'a LightClientProtocol,
    peer: PeerIndex,
    nc: &'a Arc<dyn CKBProtocolContext + Sync>,
}

impl<'a> GetBlocksProofProcess<'a> {
    pub(crate) fn new(
        message: packed::GetBlocksProofReader<'a>,
        protocol: &'a LightClientProtocol,
        peer: PeerIndex,
        nc: &'a Arc<dyn CKBProtocolContext + Sync>,
    ) -> Self {
        Self {
            message,
            protocol,
            peer,
            nc,
        }
    }

    pub(crate) async fn execute(self) -> Status {
        if self.message.block_hashes().is_empty() {
            return StatusCode::MalformedProtocolMessage.with_context("no block");
        }

        if self.message.block_hashes().len() > constant::GET_BLOCKS_PROOF_LIMIT {
            return StatusCode::MalformedProtocolMessage.with_context("too many blocks");
        }

        let snapshot = self.protocol.shared.snapshot();

        let last_block_hash = self.message.last_hash().to_entity();
        if !snapshot.is_main_chain(&last_block_hash) {
            return self
                .protocol
                .reply_tip_state::<packed::SendBlocksProof>(self.peer, self.nc)
                .await;
        }
        let last_block = snapshot
            .get_block(&last_block_hash)
            .expect("block should be in store");

        let block_hashes: Vec<_> = self
            .message
            .block_hashes()
            .to_entity()
            .into_iter()
            .collect();

        let mut uniq = HashSet::new();
        if !block_hashes
            .iter()
            .chain([last_block_hash].iter())
            .all(|hash| uniq.insert(hash))
        {
            return StatusCode::MalformedProtocolMessage
                .with_context("duplicate block hash exists");
        }

        let (found, missing): (Vec<_>, Vec<_>) = block_hashes
            .into_iter()
            .partition(|block_hash| snapshot.is_main_chain(block_hash));

        let mut positions = Vec::with_capacity(found.len());
        let mut block_headers = Vec::with_capacity(found.len());
        let mut uncles_hash = Vec::with_capacity(found.len());
        let mut extensions = Vec::with_capacity(found.len());

        for block_hash in found {
            let block = snapshot
                .get_block(&block_hash)
                .expect("block should be in store");
            positions.push(leaf_index_to_pos(block.number()));
            block_headers.push(block.header().data());
            uncles_hash.push(block.calc_uncles_hash());
            extensions.push(
                packed::BytesOpt::new_builder()
                    .set(block.extension())
                    .build(),
            );
        }

        let proved_items = (
            block_headers.into(),
            uncles_hash.into(),
            packed::BytesOptVec::new_builder().set(extensions).build(),
        );
        let missing_items = missing.into();

        self.protocol
            .reply_proof::<packed::SendBlocksProofV1>(
                self.peer,
                self.nc,
                &last_block,
                positions,
                proved_items,
                missing_items,
            )
            .await
    }
}
