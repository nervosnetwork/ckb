use crate::relayer::Relayer;
use ckb_core::transaction::Transaction;
use ckb_network::CKBProtocolContext;
use ckb_network::PeerIndex;
use ckb_protocol::{BlockTransactions, FlatbuffersVectorIterator};
use ckb_shared::index::ChainIndex;
use std::sync::Arc;

pub struct BlockTransactionsProcess<'a, CI: ChainIndex + 'a> {
    message: &'a BlockTransactions<'a>,
    relayer: &'a Relayer<CI>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a, CI> BlockTransactionsProcess<'a, CI>
where
    CI: ChainIndex + 'static,
{
    pub fn new(
        message: &'a BlockTransactions,
        relayer: &'a Relayer<CI>,
        peer: PeerIndex,
        nc: &'a CKBProtocolContext,
    ) -> Self {
        BlockTransactionsProcess {
            message,
            relayer,
            peer,
            nc,
        }
    }

    pub fn execute(self) {
        let hash = self.message.hash().unwrap().into();
        if let Some(compact_block) = self
            .relayer
            .state
            .pending_compact_blocks
            .lock()
            .remove(&hash)
        {
            let transactions: Vec<Transaction> =
                FlatbuffersVectorIterator::new(self.message.transactions().unwrap())
                    .map(Into::into)
                    .collect();

            if let Ok(block) = self.relayer.reconstruct_block(&compact_block, transactions) {
                self.relayer
                    .accept_block(self.nc, self.peer, &Arc::new(block));
            }
        }
    }
}
