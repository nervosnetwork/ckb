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
        if self.message.tx_hashes().len() > constant::GET_TRANSACTIONS_PROOF_LIMIT {
            return StatusCode::MalformedProtocolMessage.with_context("Too many transactions");
        }

        let active_chain = self.protocol.shared.active_chain();

        let last_hash = self.message.last_hash().to_entity();
        let last_block = if let Some(block) = active_chain.get_block(&last_hash) {
            block
        } else {
            return self
                .protocol
                .reply_tip_state::<packed::SendTransactionsProof>(self.peer, self.nc);
        };

        let snapshot = self.protocol.shared.shared().snapshot();

        let mut txs_in_blocks = HashMap::new();

        for tx_hash in self.message.tx_hashes().iter() {
            if let Some((tx, tx_info)) = snapshot.get_transaction_with_info(&tx_hash.to_entity()) {
                txs_in_blocks
                    .entry(tx_info.block_hash)
                    .or_insert_with(Vec::new)
                    .push((tx.data(), tx_info.index));
            }
        }

        let positions: Vec<_> = txs_in_blocks
            .keys()
            .filter_map(|hash| {
                active_chain
                    .get_block_header(hash)
                    .map(|header| header.number())
            })
            .filter(|number| last_block.number() != *number)
            .filter_map(|number| active_chain.get_ancestor(&last_hash, number))
            .map(|header| leaf_index_to_pos(header.number()))
            .collect();

        let filtered_blocks: Vec<_> = txs_in_blocks
            .into_iter()
            .filter_map(|(block_hash, txs_and_tx_indices)| {
                snapshot.get_block(&block_hash).map(|block| {
                    let merkle_proof = CBMT::build_merkle_proof(
                        &block
                            .transactions()
                            .iter()
                            .map(|tx| tx.hash())
                            .collect::<Vec<_>>(),
                        &txs_and_tx_indices
                            .iter()
                            .map(|(_tx, index)| *index as u32)
                            .collect::<Vec<_>>(),
                    )
                    .expect("build proof with verified inputs should be OK");

                    let txs: Vec<_> = txs_and_tx_indices.into_iter().map(|(tx, _)| tx).collect();

                    packed::FilteredBlock::new_builder()
                        .header(block.header().data())
                        .witnesses_root(block.calc_witnesses_root())
                        .transactions(txs.pack())
                        .proof(
                            packed::MerkleProof::new_builder()
                                .indices(merkle_proof.indices().to_owned().pack())
                                .lemmas(merkle_proof.lemmas().to_owned().pack())
                                .build(),
                        )
                        .build()
                })
            })
            .collect();

        let proved_items = packed::FilteredBlockVec::new_builder()
            .set(filtered_blocks)
            .build();

        self.protocol.reply_proof::<packed::SendTransactionsProof>(
            self.peer,
            self.nc,
            &last_block,
            positions,
            proved_items,
        )
    }
}
