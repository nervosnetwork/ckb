use crate::relayer::block_transactions_verifier::BlockTransactionsVerifier;
use crate::relayer::Relayer;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{core, packed, prelude::*};
use failure::Error as FailureError;
use std::collections::hash_map::Entry;
use std::sync::Arc;

pub struct BlockTransactionsProcess<'a> {
    message: packed::BlockTransactionsReader<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

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
        let block_hash = self.message.block_hash().to_entity();
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

                let transactions: Vec<core::TransactionView> = self
                    .message
                    .transactions()
                    .to_entity()
                    .into_iter()
                    .map(|tx| tx.into_view())
                    .collect();

                BlockTransactionsVerifier::verify(&compact_block, &indexes, &transactions)?;

                let ret = self.relayer.reconstruct_block(compact_block, transactions);

                // TODO Add this (compact_block, peer) into RecentRejects if reconstruct_block failed?
                // TODO Add this block into RecentRejects if accept_block failed?
                if let Ok(block) = ret {
                    pending.remove();
                    self.relayer
                        .accept_block(self.nc.as_ref(), self.peer, block);
                    return Ok(Status::Accept);
                }
                return Ok(Status::Missing);
            }
        }

        Ok(Status::UnkownRequest)
    }
}
