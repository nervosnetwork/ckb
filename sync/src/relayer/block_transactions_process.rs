use crate::relayer::Relayer;
use ckb_core::transaction::Transaction;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, BlockTransactions, FlatbuffersVectorIterator};
use ckb_store::ChainStore;
use failure::Error as FailureError;
use std::collections::hash_map::Entry;
use std::convert::TryInto;
use std::sync::Arc;

pub struct BlockTransactionsProcess<'a, CS> {
    message: &'a BlockTransactions<'a>,
    relayer: &'a Relayer<CS>,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a, CS: ChainStore + 'static> BlockTransactionsProcess<'a, CS> {
    pub fn new(
        message: &'a BlockTransactions,
        relayer: &'a Relayer<CS>,
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

    pub fn execute(self) -> Result<(), FailureError> {
        let block_hash = cast!(self.message.block_hash())?.try_into()?;
        if let Entry::Occupied(mut pending) = self
            .relayer
            .shared()
            .pending_compact_blocks()
            .entry(block_hash)
        {
            let (compact_block, peers_set) = pending.get_mut();
            if peers_set.remove(&self.peer) {
                let transactions: Vec<Transaction> =
                    FlatbuffersVectorIterator::new(cast!(self.message.transactions())?)
                        .map(TryInto::try_into)
                        .collect::<Result<_, FailureError>>()?;

                let ret = {
                    let chain_state = self.relayer.shared.lock_chain_state();
                    self.relayer
                        .reconstruct_block(&chain_state, compact_block, transactions)
                };

                // TODO Add this (compact_block, peer) into RecentRejects if reconstruct_block failed?
                // TODO Add this block into RecentRejects if accept_block failed?
                if let Ok(block) = ret {
                    pending.remove();
                    self.relayer
                        .accept_block(self.nc.as_ref(), self.peer, block);
                }
            }
        }

        Ok(())
    }
}
