use crate::relayer::Relayer;
use ckb_core::transaction::Transaction;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, BlockTransactions, FlatbuffersVectorIterator};
use ckb_store::ChainStore;
use failure::Error as FailureError;
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
        if let Some(compact_block) = self
            .relayer
            .pending_compact_blocks
            .lock()
            .remove(&block_hash)
        {
            let transactions: Vec<Transaction> =
                FlatbuffersVectorIterator::new(cast!(self.message.transactions())?)
                    .map(TryInto::try_into)
                    .collect::<Result<_, FailureError>>()?;

            let ret = {
                let chain_state = self.relayer.shared.lock_chain_state();
                self.relayer
                    .reconstruct_block(&chain_state, &compact_block, transactions)
            };

            if let Ok(block) = ret {
                self.relayer
                    .accept_block(self.nc.as_ref(), self.peer, &Arc::new(block));
            }
        }
        Ok(())
    }
}
