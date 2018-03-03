use block::Block;
use transaction::Transaction;

pub trait ChainAdapter: Sync + Send {
    fn block_accepted(&self, b: &Block);
}

pub trait NetAdapter: Sync + Send {
    fn block_received(&self, b: Block);
    fn transaction_received(&self, tx: Transaction);
}
