use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_chain::PowEngine;
use ckb_protocol::{BlockTransactions, FlatbuffersVectorIterator};
use core::transaction::IndexedTransaction;
use network::PeerId;
use relayer::Relayer;

pub struct BlockTransactionsProcess<'a, C: 'a, P: 'a> {
    message: &'a BlockTransactions<'a>,
    relayer: &'a Relayer<C, P>,
    peer: PeerId,
}

impl<'a, C, P> BlockTransactionsProcess<'a, C, P>
where
    C: ChainProvider + 'static,
    P: PowEngine + 'static,
{
    pub fn new(message: &'a BlockTransactions, relayer: &'a Relayer<C, P>, peer: PeerId) -> Self {
        BlockTransactionsProcess {
            message,
            relayer,
            peer,
        }
    }

    pub fn execute(self) {
        let hash = H256::from_slice(self.message.hash().and_then(|b| b.seq()).unwrap());
        if let Some(compact_block) = self
            .relayer
            .state
            .pending_compact_blocks
            .lock()
            .remove(&hash)
        {
            let transactions: Vec<IndexedTransaction> =
                FlatbuffersVectorIterator::new(self.message.transactions().unwrap())
                    .map(Into::into)
                    .collect();

            if let (Some(block), _) = self.relayer.reconstruct_block(&compact_block, transactions) {
                let _ = self.relayer.accept_block(self.peer, &block);
            }
        }
    }
}
