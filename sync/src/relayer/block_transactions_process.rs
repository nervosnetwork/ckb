use crate::relayer::Relayer;
use ckb_core::transaction::Transaction;
use ckb_network::{CKBProtocolContext, SessionId};
use ckb_protocol::{cast, BlockTransactions, FlatbuffersVectorIterator};
use ckb_shared::store::ChainStore;
use failure::Error as FailureError;
use std::convert::TryInto;
use std::sync::Arc;

pub struct BlockTransactionsProcess<'a, CS> {
    message: &'a BlockTransactions<'a>,
    relayer: &'a Relayer<CS>,
    peer: SessionId,
    nc: &'a mut CKBProtocolContext,
}

impl<'a, CS: ChainStore> BlockTransactionsProcess<'a, CS> {
    pub fn new(
        message: &'a BlockTransactions,
        relayer: &'a Relayer<CS>,
        peer: SessionId,
        nc: &'a mut CKBProtocolContext,
    ) -> Self {
        BlockTransactionsProcess {
            message,
            relayer,
            peer,
            nc,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let hash = cast!(self.message.hash())?.try_into()?;
        if let Some(compact_block) = self
            .relayer
            .state
            .pending_compact_blocks
            .lock()
            .remove(&hash)
        {
            let transactions: Result<Vec<Transaction>, FailureError> =
                FlatbuffersVectorIterator::new(cast!(self.message.transactions())?)
                    .map(TryInto::try_into)
                    .collect();

            let ret = {
                let chain_state = self.relayer.shared.chain_state().lock();
                self.relayer
                    .reconstruct_block(&chain_state, &compact_block, transactions?)
            };

            if let Ok(block) = ret {
                self.relayer
                    .accept_block(self.nc, self.peer, &Arc::new(block));
            }
        }
        Ok(())
    }
}
