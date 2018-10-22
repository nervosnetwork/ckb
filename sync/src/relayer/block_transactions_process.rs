use bigint::H256;
use ckb_protocol::{BlockTransactions, FlatbuffersVectorIterator};
use ckb_shared::index::ChainIndex;
use core::transaction::Transaction;
use network::PeerIndex;
use relayer::Relayer;

pub struct BlockTransactionsProcess<'a, CI: ChainIndex + 'a> {
    message: &'a BlockTransactions<'a>,
    relayer: &'a Relayer<CI>,
    peer: PeerIndex,
}

impl<'a, CI> BlockTransactionsProcess<'a, CI>
where
    CI: ChainIndex + 'static,
{
    pub fn new(message: &'a BlockTransactions, relayer: &'a Relayer<CI>, peer: PeerIndex) -> Self {
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
            let transactions: Vec<Transaction> =
                FlatbuffersVectorIterator::new(self.message.transactions().unwrap())
                    .map(Into::into)
                    .collect();

            if let (Some(block), _) = self.relayer.reconstruct_block(&compact_block, transactions) {
                let _ = self.relayer.accept_block(self.peer, block);
            }
        }
    }
}
