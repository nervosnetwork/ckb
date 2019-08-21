use crate::relayer::block_transactions_verifier::BlockTransactionsVerifier;
use crate::relayer::error::{Error, Misbehavior};
use crate::relayer::{ReconstructionError, Relayer};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{core, packed, prelude::*};
use failure::Error as FailureError;
use std::collections::hash_map::Entry;
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
        let block_transactions = self.message.to_entity();
        let block_hash = block_transactions.block_hash();
        let transactions: Vec<core::TransactionView> = block_transactions
            .transactions()
            .into_iter()
            .map(|tx| tx.into_view())
            .collect();

        let missing_indexes: Vec<u32>;
        let mut collision = false;

        if let Entry::Occupied(mut pending) = self
            .relayer
            .shared()
            .pending_compact_blocks()
            .entry(block_hash.clone())
        {
            let (compact_block, peers_map) = pending.get_mut();
            if let Some(indexes) = peers_map.remove(&self.peer) {
                ckb_logger::info!(
                    "realyer receive BLOCKTXN of {}, peer: {}",
                    block_hash,
                    self.peer
                );

                BlockTransactionsVerifier::verify(&compact_block, &indexes, &transactions)?;

                let ret = self.relayer.reconstruct_block(compact_block, transactions);

                match ret {
                    Ok(block) => {
                        pending.remove();
                        self.relayer
                            .accept_block(self.nc.as_ref(), self.peer, block);
                        return Ok(Status::Accept);
                    }
                    Err(ReconstructionError::InvalidTransactionRoot) => {
                        return Err(Error::Misbehavior(Misbehavior::InvalidTransactionRoot).into());
                    }
                    Err(ReconstructionError::MissingIndexes(missing)) => {
                        missing_indexes = missing.into_iter().map(|i| i as u32).collect();
                    }
                    Err(ReconstructionError::Collision) => {
                        missing_indexes = compact_block
                            .short_id_indexes()
                            .into_iter()
                            .map(|i| i as u32)
                            .collect();
                        collision = true;
                    }
                }

                assert!(!missing_indexes.is_empty());

                let content = packed::GetBlockTransactions::new_builder()
                    .block_hash(block_hash)
                    .indexes(missing_indexes.pack())
                    .build();
                let message = packed::RelayMessage::new_builder().set(content).build();
                let data = message.as_slice().into();
                if let Err(err) = self.nc.send_message_to(self.peer, data) {
                    ckb_logger::debug!("relayer send get_block_transactions error: {:?}", err);
                }

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
