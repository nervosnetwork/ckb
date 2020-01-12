use crate::relayer::block_transactions_verifier::BlockTransactionsVerifier;
use crate::relayer::block_uncles_verifier::BlockUnclesVerifier;
use crate::relayer::error::{Error, Internal, Misbehavior};
use crate::relayer::{ReconstructionError, Relayer};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{core, packed, prelude::*};
use failure::Error as FailureError;
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
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Status {
    // BlockTransactionsVerifier has checked it,
    // so shoud not reach here unless the peer loses the transactions in tx_pool
    Missing,
    // The BlockTransaction message includes all the requested transactions.
    // The peer recovers the whole block and accept it.
    Accept,
    // The peer may lose it's cache
    // or other peer makes some mistakes
    UnkownRequest,
    // Maybe short_id collides, re-send get_block_transactions message
    CollisionAndSendMissingIndexes,
}

impl<'a> BlockTransactionsProcess<'a> {
    pub fn new(
        message: packed::BlockTransactionsReader<'a>,
        relayer: &'a Relayer,
        nc: Arc<dyn CKBProtocolContext>,
        peer: PeerIndex,
    ) -> Self {
        BlockTransactionsProcess {
            message,
            relayer,
            nc,
            peer,
        }
    }

    pub fn execute(self) -> Result<Status, FailureError> {
        let snapshot = self.relayer.shared().snapshot();
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

        let missing_transactions: Vec<u32>;
        let missing_uncles: Vec<u32>;
        let mut collision = false;

        if let Entry::Occupied(mut pending) = snapshot
            .state()
            .pending_compact_blocks()
            .entry(block_hash.clone())
        {
            let (compact_block, peers_map) = pending.get_mut();
            if let Entry::Occupied(mut value) = peers_map.entry(self.peer) {
                let (expected_transaction_indexes, expected_uncle_indexes) = value.get_mut();
                ckb_logger::info!(
                    "realyer receive BLOCKTXN of {}, peer: {}",
                    block_hash,
                    self.peer
                );

                BlockTransactionsVerifier::verify(
                    &compact_block,
                    &expected_transaction_indexes,
                    &received_transactions,
                )?;
                BlockUnclesVerifier::verify(
                    &compact_block,
                    &expected_uncle_indexes,
                    &received_uncles,
                )?;

                let ret = self.relayer.reconstruct_block(
                    &snapshot,
                    compact_block,
                    received_transactions,
                    &expected_uncle_indexes,
                    &received_uncles,
                );

                // Request proposal
                let proposals: Vec<_> = received_uncles
                    .into_iter()
                    .flat_map(|u| u.data().proposals().into_iter())
                    .collect();
                if let Err(err) = self.relayer.request_proposal_txs(
                    self.nc.as_ref(),
                    self.peer,
                    block_hash.clone(),
                    proposals,
                ) {
                    debug_target!(
                        crate::LOG_TARGET_RELAY,
                        "[BlockTransactionsProcess] request_proposal_txs: {}",
                        err
                    );
                };

                match ret {
                    Ok(block) => {
                        pending.remove();
                        self.relayer
                            .accept_block(&snapshot, self.nc.as_ref(), self.peer, block);
                        return Ok(Status::Accept);
                    }
                    Err(ReconstructionError::InvalidTransactionRoot) => {
                        return Err(Error::Misbehavior(Misbehavior::InvalidTransactionRoot).into());
                    }
                    Err(ReconstructionError::InvalidUncle) => {
                        return Err(Error::Misbehavior(Misbehavior::InvalidUncle).into());
                    }
                    Err(ReconstructionError::MissingIndexes(transactions, uncles)) => {
                        missing_transactions = transactions.into_iter().map(|i| i as u32).collect();
                        missing_uncles = uncles.into_iter().map(|i| i as u32).collect();
                    }
                    Err(ReconstructionError::Collision) => {
                        missing_transactions = compact_block
                            .short_id_indexes()
                            .into_iter()
                            .map(|i| i as u32)
                            .collect();
                        collision = true;
                        missing_uncles = vec![];
                    }
                    Err(ReconstructionError::Internal(e)) => {
                        ckb_logger::error!("reconstruct_block internal error: {}", e);
                        return Err(Error::Internal(Internal::TxPoolInternalError).into());
                    }
                }

                assert!(!missing_transactions.is_empty() || !missing_uncles.is_empty());

                let content = packed::GetBlockTransactions::new_builder()
                    .block_hash(block_hash)
                    .indexes(missing_transactions.pack())
                    .uncle_indexes(missing_uncles.pack())
                    .build();
                let message = packed::RelayMessage::new_builder().set(content).build();
                let data = message.as_slice().into();
                if let Err(err) = self.nc.send_message_to(self.peer, data) {
                    ckb_logger::debug!("relayer send get_block_transactions error: {:?}", err);
                }

                mem::replace(expected_transaction_indexes, missing_transactions);
                mem::replace(expected_uncle_indexes, missing_uncles);

                if collision {
                    return Ok(Status::CollisionAndSendMissingIndexes);
                } else {
                    return Ok(Status::Missing);
                }
            }
        }

        Ok(Status::UnkownRequest)
    }
}
