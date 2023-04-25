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

        let last_hash = self.message.last_hash().to_entity();
        let last_block = if let Some(block) = snapshot.get_block(&last_hash) {
            block
        } else {
            return self
                .protocol
                .reply_tip_state::<packed::SendTransactionsProof>(self.peer, self.nc);
        };

        let (txs_in_blocks, missing_txs) = self
            .message
            .tx_hashes()
            .to_entity()
            .into_iter()
            .map(|tx_hash| {
                let tx_with_info = snapshot.get_transaction_with_info(&tx_hash);
                (tx_hash, tx_with_info)
            })
            .fold(
                (HashMap::new(), Vec::new()),
                |(mut found, mut missing_txs), (tx_hash, tx_with_info)| {
                    if let Some((tx, tx_info)) = tx_with_info {
                        found
                            .entry(tx_info.block_hash)
                            .or_insert_with(Vec::new)
                            .push((tx, tx_info.index));
                    } else {
                        missing_txs.push(tx_hash);
                    }
                    (found, missing_txs)
                },
            );

        let (positions, filtered_blocks, missing_txs) = txs_in_blocks
            .into_iter()
            .map(|(block_hash, txs_and_tx_indices)| {
                snapshot
                    .get_block_header(&block_hash)
                    .map(|header| header.number())
                    .filter(|number| *number != last_block.number())
                    .and_then(|number| snapshot.get_ancestor(&last_hash, number))
                    .filter(|header| header.hash() == block_hash)
                    .and_then(|_| snapshot.get_block(&block_hash))
                    .map(|block| (block, txs_and_tx_indices.clone()))
                    .ok_or_else(|| {
                        txs_and_tx_indices
                            .into_iter()
                            .map(|(tx, _)| tx.hash())
                            .collect::<Vec<_>>()
                    })
            })
            .fold(
                (Vec::new(), Vec::new(), missing_txs),
                |(mut positions, mut filtered_blocks, mut missing_txs), result| {
                    match result {
                        Ok((block, txs_and_tx_indices)) => {
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
                        }
                        Err(tx_hashes) => {
                            missing_txs.extend(tx_hashes);
                        }
                    }
                    (positions, filtered_blocks, missing_txs)
                },
            );

        let proved_items = packed::FilteredBlockVec::new_builder()
            .set(filtered_blocks)
            .build();
        let missing_items = missing_txs.pack();

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
