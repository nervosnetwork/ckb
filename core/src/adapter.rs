use block::Block;
use transaction::Transaction;

pub trait ChainAdapter {
    fn block_accepted(&self, b: &Block);
}

pub trait NetAdapter {
    fn block_received(&self, b: Block);
    fn transaction_received(&self, tx: Transaction);
}
