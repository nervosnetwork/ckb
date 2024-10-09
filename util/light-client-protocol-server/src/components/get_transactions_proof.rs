use std::collections::HashMap;

use ckb_merkle_mountain_range::leaf_index_to_pos;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_store::ChainStore;
use ckb_types::{packed, prelude::*, utilities::CBMT};

use crate::{constant, LightClientProtocol, Status, StatusCode};

pub(crate) struct GetTransactionsProofProcess<'a> {
    message: packed::GetTransactionsProofReader<'a>,
    protocol: &'a LightClientProtocol,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

impl<'a> GetTransactionsProofProcess<'a> {
    pub(crate) fn new(
        message: packed::GetTransactionsProofReader<'a>,
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
        if self.message.tx_hashes().is_empty() {
            return StatusCode::MalformedProtocolMessage.with_context("no transaction");
        }

        if self.message.tx_hashes().len() > constant::GET_TRANSACTIONS_PROOF_LIMIT {
            return StatusCode::MalformedProtocolMessage.with_context("too many transactions");
        }

        let snapshot = self.protocol.shared.snapshot();

        let last_block_hash = self.message.last_hash().to_entity();
        if !snapshot.is_main_chain(&last_block_hash) {
            return self
                .protocol
                .reply_tip_state::<packed::SendTransactionsProof>(self.peer, self.nc);
        }
        let last_block = snapshot
            .get_block(&last_block_hash)
            .expect("block should be in store");

        let (found, missing): (Vec<_>, Vec<_>) = self
            .message
            .tx_hashes()
            .to_entity()
            .into_iter()
            .partition(|tx_hash| {
                snapshot
                    .get_transaction_info(tx_hash)
                    .map(|tx_info| snapshot.is_main_chain(&tx_info.block_hash))
                    .unwrap_or_default()
            });

        let mut txs_in_blocks = HashMap::new();
        for tx_hash in found {
            let (tx, tx_info) = snapshot
                .get_transaction_with_info(&tx_hash)
                .expect("tx exists");
            txs_in_blocks
                .entry(tx_info.block_hash)
                .or_insert_with(Vec::new)
                .push((tx, tx_info.index));
        }

        let mut positions = Vec::with_capacity(txs_in_blocks.len());
        let mut filtered_blocks = Vec::with_capacity(txs_in_blocks.len());
        let mut uncles_hash = Vec::with_capacity(txs_in_blocks.len());
        let mut extensions = Vec::with_capacity(txs_in_blocks.len());
        let ckb2023 = self.nc.ckb2023();

        for (block_hash, txs_and_tx_indices) in txs_in_blocks.into_iter() {
            let block = snapshot
                .get_block(&block_hash)
                .expect("block should be in store");
            let merkle_proof = CBMT::build_merkle_proof(
                &block
                    .transactions()
                    .iter()
                    .map(|tx| tx.hash())
                    .collect::<Vec<_>>(),
                &txs_and_tx_indices
                    .iter()
                    .map(|(_, index)| *index as u32)
                    .collect::<Vec<_>>(),
            )
            .expect("build proof with verified inputs should be OK");

            let txs: Vec<_> = txs_and_tx_indices
                .into_iter()
                .map(|(tx, _)| tx.data())
                .collect();

            let filtered_block = packed::FilteredBlock::new_builder()
                .header(block.header().data())
                .witnesses_root(block.calc_witnesses_root())
                .transactions(txs.pack())
                .proof(
                    packed::MerkleProof::new_builder()
                        .indices(merkle_proof.indices().to_owned().pack())
                        .lemmas(merkle_proof.lemmas().to_owned().pack())
                        .build(),
                )
                .build();

            positions.push(leaf_index_to_pos(block.number()));
            filtered_blocks.push(filtered_block);
            if ckb2023 {
                let uncles = snapshot
                    .get_block_uncles(&block_hash)
                    .expect("block uncles must be stored");
                let extension = snapshot.get_block_extension(&block_hash);

                uncles_hash.push(uncles.data().calc_uncles_hash());
                extensions.push(packed::BytesOpt::new_builder().set(extension).build());
            }
        }

        if ckb2023 {
            let proved_items = (
                packed::FilteredBlockVec::new_builder()
                    .set(filtered_blocks)
                    .build(),
                uncles_hash.pack(),
                packed::BytesOptVec::new_builder().set(extensions).build(),
            );
            let missing_items = missing.pack();

            self.protocol
                .reply_proof::<packed::SendTransactionsProofV1>(
                    self.peer,
                    self.nc,
                    &last_block,
                    positions,
                    proved_items,
                    missing_items,
                )
        } else {
            let proved_items = packed::FilteredBlockVec::new_builder()
                .set(filtered_blocks)
                .build();
            let missing_items = missing.pack();

            self.protocol.reply_proof::<packed::SendTransactionsProof>(
                self.peer,
                self.nc,
                &last_block,
                positions,
                proved_items,
                missing_items,
            )
        }
    }
}
