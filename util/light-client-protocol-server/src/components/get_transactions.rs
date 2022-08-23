use std::collections::HashMap;

use ckb_merkle_mountain_range::leaf_index_to_pos;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_store::ChainStore;
use ckb_types::{packed, prelude::*, utilities::CBMT};

use crate::{prelude::*, LightClientProtocol, Status, StatusCode};

const MAX_TRANSACTIONS_SIZE: usize = 1000;

pub(crate) struct GetTransactionsProcess<'a> {
    message: packed::GetTransactionsReader<'a>,
    protocol: &'a LightClientProtocol,
    peer: PeerIndex,
    nc: &'a dyn CKBProtocolContext,
}

impl<'a> GetTransactionsProcess<'a> {
    pub(crate) fn new(
        message: packed::GetTransactionsReader<'a>,
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
        let content = packed::SendTransactions::new_builder()
            .tip_header(tip_header)
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();
        self.nc.reply(self.peer, &message);
        Status::ok()
    }

    pub(crate) fn execute(self) -> Status {
        if self.message.tx_hashes().len() > MAX_TRANSACTIONS_SIZE {
            return StatusCode::MalformedProtocolMessage.with_context("Too many transactions");
        }

        let active_chain = self.protocol.shared.active_chain();
        let snapshot = self.protocol.shared.shared().snapshot();

        let tip_hash = self.message.tip_hash().to_entity();

        let tip_block = if let Some(block) = active_chain.get_block(&tip_hash) {
            block
        } else {
            return self.reply_only_the_tip_state();
        };

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
            .filter(|number| tip_block.number() != *number)
            .filter_map(|number| active_chain.get_ancestor(&tip_hash, number))
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
        let block_proof = match mmr.gen_proof(positions) {
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

        let content = packed::SendTransactions::new_builder()
            .block_proof(block_proof.pack())
            .tip_header(verifiable_tip_header)
            .filtered_blocks(
                packed::FilteredBlockVec::new_builder()
                    .set(filtered_blocks)
                    .build(),
            )
            .build();
        let message = packed::LightClientMessage::new_builder()
            .set(content)
            .build();
        self.nc.reply(self.peer, &message);

        Status::ok()
    }
}
