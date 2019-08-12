use crate::relayer::block_transactions_verifier::BlockTransactionsVerifier;
use crate::relayer::Relayer;
use crate::{attempt, Status, StatusCode};
use ckb_core::transaction::Transaction;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, BlockTransactions, FlatbuffersVectorIterator};
use failure::Error as FailureError;
use numext_fixed_hash::H256;
use std::collections::hash_map::Entry;
use std::convert::TryInto;
use std::sync::Arc;

pub struct BlockTransactionsProcess<'a> {
    message: &'a BlockTransactions<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a> BlockTransactionsProcess<'a> {
    pub fn new(
        message: &'a BlockTransactions,
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

    pub fn execute(self) -> Status {
        let block_hash = attempt!(TryInto::<H256>::try_into(attempt!(cast!(self
            .message
            .block_hash()))));
        if let Entry::Occupied(mut pending) = self
            .relayer
            .shared()
            .pending_compact_blocks()
            .entry(block_hash.clone())
        {
            let (compact_block, peers_map) = pending.get_mut();
            if let Some(indexes) = peers_map.remove(&self.peer) {
                ckb_logger::info!(
                    "realyer receive BLOCKTXN of {:#x}, peer: {}",
                    block_hash,
                    self.peer
                );

                let result: Result<Vec<Transaction>, _> =
                    FlatbuffersVectorIterator::new(attempt!(cast!(self.message.transactions())))
                        .map(TryInto::try_into)
                        .collect::<Result<_, FailureError>>();
                let transactions = attempt!(result);

                attempt!(BlockTransactionsVerifier::verify(
                    &compact_block,
                    &indexes,
                    &transactions
                ));

                let ret = self.relayer.reconstruct_block(compact_block, transactions);

                // TODO Add this (compact_block, peer) into RecentRejects if reconstruct_block failed?
                // TODO Add this block into RecentRejects if accept_block failed?
                if let Ok(block) = ret {
                    pending.remove();
                    self.relayer
                        .accept_block(self.nc.as_ref(), self.peer, block);
                    return Status::ok();
                }
                return StatusCode::WaitingTransactions.into();
            }
        }

        Status::ok()
    }
}
