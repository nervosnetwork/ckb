use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_protocol::{BlockTransactions, FlatbuffersVectorIterator};
use core::transaction::IndexedTransaction;
use network::PeerId;
use relayer::Relayer;

pub struct BlockTransactionsProcess<'a, C: 'a> {
    message: &'a BlockTransactions<'a>,
    relayer: &'a Relayer<C>,
    peer: PeerId,
}

impl<'a, C> BlockTransactionsProcess<'a, C>
where
    C: ChainProvider + 'static,
{
    pub fn new(message: &'a BlockTransactions, relayer: &'a Relayer<C>, peer: PeerId) -> Self {
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
