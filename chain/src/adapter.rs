use chain::Chain;
use core::block::Block;
use core::transaction::Transaction;
use network::Network;
use pool::{OrphanBlockPool, TransactionPool};

pub trait ChainAdapter {
    fn block_accepted(&self, b: &Block);
}

pub struct ChainToNetAndPoolAdapter {
    pub orphan_pool: Box<OrphanBlockPool>,
    pub network: Box<Network>,
}

impl ChainAdapter for ChainToNetAndPoolAdapter {
    fn block_accepted(&self, b: &Block) {
        self.orphan_pool.add_block(b);
        self.network.broadcast(b)
    }
}

pub trait NetAdapter {
    fn block_received(&self, b: Block);
    fn transaction_received(&self, tx: Transaction);
}

pub struct NetToChainAndPoolAdapter {
    pub chain: Box<Chain>,
    pub tx_pool: Box<TransactionPool>,
}

impl NetAdapter for NetToChainAndPoolAdapter {
    fn block_received(&self, b: Block) {
        if b.validate() {
            self.chain.process_block(&b)
        } else {
            // TODO ban remote peer
        }
    }

    fn transaction_received(&self, tx: Transaction) {
        if tx.validate() {
            self.tx_pool.add_transaction(tx)
        } else {
            // TODO ban remote peer
        }
    }
}
