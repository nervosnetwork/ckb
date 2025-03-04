use crate::relayer::block_transactions_verifier::BlockTransactionsVerifier;
use crate::relayer::block_uncles_verifier::BlockUnclesVerifier;
use crate::relayer::{ReconstructionResult, Relayer};
use crate::utils::send_message_to;
use crate::{Status, StatusCode, attempt};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{core, packed, prelude::*};
use std::collections::hash_map::Entry;
use std::mem;
use std::sync::Arc;

// Keeping in mind that short_ids are expected to occasionally collide.
// On receiving block-transactions message,
// while the reconstructed the block has a different transactions_root,
// 1. If the BlockTransactions includes all the transactions matched short_ids in the compact block,
// In this situation, the peer sends all the transactions by either prefilled or block-transactions,
// no one transaction from the tx-pool or store,
// the node should ban the peer but not mark the block invalid
// because of the block hash may be wrong.
// 2. If not all the transactions comes from the peer,
// there may be short_id collision in transaction pool.
// the node retreat to request all the short_ids from the peer.
pub struct BlockTransactionsProcess<'a> {
    message: packed::BlockTransactionsReader<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext + Sync>,
    peer: PeerIndex,
}

impl<'a> BlockTransactionsProcess<'a> {
    pub fn new(
        message: packed::BlockTransactionsReader<'a>,
        relayer: &'a Relayer,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
    ) -> Self {
        BlockTransactionsProcess {
            message,
            relayer,
            nc,
            peer,
        }
    }

    pub fn execute(self) -> Status {
        let shared = self.relayer.shared();
        let active_chain = shared.active_chain();
        let block_transactions = self.message.to_entity();
        let block_hash = block_transactions.block_hash();
        let received_transactions: Vec<core::TransactionView> = block_transactions
            .transactions()
            .into_iter()
            .map(|tx| tx.into_view())
            .collect();
        let received_uncles: Vec<core::UncleBlockView> = block_transactions
            .uncles()
            .into_iter()
            .map(|uncle| uncle.into_view())
            .collect();

        let mut missing_transactions: Vec<u32>;
        let mut missing_uncles: Vec<u32>;
        let mut collision = false;

        if let Entry::Occupied(mut pending) = shared
            .state()
            .pending_compact_blocks()
            .entry(block_hash.clone())
        {
            let (compact_block, peers_map, _) = pending.get_mut();
            if let Entry::Occupied(mut value) = peers_map.entry(self.peer) {
                let (expected_transaction_indexes, expected_uncle_indexes) = value.get_mut();
                ckb_logger::info!(
                    "relayer receive BLOCKTXN of {}, peer: {}",
                    block_hash,
                    self.peer
                );

                attempt!(BlockTransactionsVerifier::verify(
                    compact_block,
                    expected_transaction_indexes,
                    &received_transactions,
                ));
                attempt!(BlockUnclesVerifier::verify(
                    compact_block,
                    expected_uncle_indexes,
                    &received_uncles,
                ));

                let ret = self.relayer.reconstruct_block(
                    &active_chain,
                    compact_block,
                    received_transactions,
                    expected_uncle_indexes,
                    &received_uncles,
                );

                // Request proposal
                {
                    let proposals: Vec<_> = received_uncles
                        .into_iter()
                        .flat_map(|u| u.data().proposals().into_iter())
                        .collect();
                    self.relayer.request_proposal_txs(
                        self.nc.as_ref(),
                        self.peer,
                        (
                            compact_block.header().into_view().number(),
                            block_hash.clone(),
                        )
                            .into(),
                        proposals,
                    );
                }

                match ret {
                    ReconstructionResult::Block(block) => {
                        pending.remove();
                        self.relayer
                            .accept_block(self.nc, self.peer, block, "BlockTransactions");
                        return Status::ok();
                    }
                    ReconstructionResult::Missing(transactions, uncles) => {
                        // We need to get all transactions and uncles that do not exist locally
                        // at once to restore a block
                        //
                        // Under normal circumstances, one request is enough, when the chain occurs fork,
                        // the transaction pool may drop some transactions due to double spend check, at
                        // this time, the previously issued request to obtain transactions may not meet
                        // the needs of a one-time construction, we need to send another complete request
                        // to do so. That is, the current miss + the miss of the previous request are
                        // combined and requested once. This is a small probability event
                        missing_transactions = transactions
                            .into_iter()
                            .map(|i| i as u32)
                            .chain(expected_transaction_indexes.iter().copied())
                            .collect();
                        missing_uncles = uncles
                            .into_iter()
                            .map(|i| i as u32)
                            .chain(expected_uncle_indexes.iter().copied())
                            .collect();

                        missing_transactions.sort_unstable();
                        missing_uncles.sort_unstable();
                    }
                    ReconstructionResult::Collided => {
                        missing_transactions = compact_block
                            .short_id_indexes()
                            .into_iter()
                            .map(|i| i as u32)
                            .collect();
                        collision = true;
                        missing_uncles = vec![];
                    }
                    ReconstructionResult::Error(status) => {
                        return status;
                    }
                }

                assert!(!missing_transactions.is_empty() || !missing_uncles.is_empty());

                let content = packed::GetBlockTransactions::new_builder()
                    .block_hash(block_hash.clone())
                    .indexes(missing_transactions.pack())
                    .uncle_indexes(missing_uncles.pack())
                    .build();
                let message = packed::RelayMessage::new_builder().set(content).build();

                attempt!(send_message_to(self.nc.as_ref(), self.peer, &message));

                let _ignore_prev_value =
                    mem::replace(expected_transaction_indexes, missing_transactions);
                let _ignore_prev_value = mem::replace(expected_uncle_indexes, missing_uncles);

                if collision {
                    return StatusCode::CompactBlockMeetsShortIdsCollision.with_context(block_hash);
                } else {
                    return StatusCode::CompactBlockRequiresFreshTransactions
                        .with_context(block_hash);
                }
            }
        }

        Status::ignored()
    }
}
