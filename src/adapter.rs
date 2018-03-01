use chain::chain::Chain;
use core::adapter::{ChainAdapter, NetAdapter};
use core::block::Block;
use core::transaction::Transaction;
use network::Network;
use pool::{OrphanBlockPool, TransactionPool};
use std::cell::{Ref, RefCell};
use std::ops::Deref;
use std::sync::{Arc, RwLock, Weak};

// helper function
fn w<T>(weak: &RefCell<Option<Weak<T>>>) -> Arc<T> {
    let r = Ref::map(weak.borrow(), |o| o.as_ref().unwrap());
    r.deref().upgrade().unwrap()
}

#[derive(Clone)]
pub struct ChainToNetAndPoolAdapter {
    orphan_pool: Arc<RwLock<OrphanBlockPool>>,
    network: RefCell<Option<Weak<Network>>>,
}

impl Default for ChainToNetAndPoolAdapter {
    fn default() -> ChainToNetAndPoolAdapter {
        ChainToNetAndPoolAdapter {
            orphan_pool: Arc::new(RwLock::new(OrphanBlockPool {})),
            network: RefCell::new(None),
        }
    }
}

impl ChainAdapter for ChainToNetAndPoolAdapter {
    fn block_accepted(&self, b: &Block) {
        self.orphan_pool.write().unwrap().add_block(b);
        w(&self.network).broadcast(b)
    }
}

impl ChainToNetAndPoolAdapter {
    pub fn init(&self, network: Weak<Network>) {
        let mut inner = self.network.borrow_mut();
        *inner = Some(network)
    }
}

#[derive(Clone)]
pub struct NetToChainAndPoolAdapter {
    tx_pool: Arc<TransactionPool>,
    chain: RefCell<Option<Weak<Chain>>>,
}

impl Default for NetToChainAndPoolAdapter {
    fn default() -> NetToChainAndPoolAdapter {
        NetToChainAndPoolAdapter {
            tx_pool: Arc::new(TransactionPool::default()),
            chain: RefCell::new(None),
        }
    }
}

impl NetAdapter for NetToChainAndPoolAdapter {
    fn block_received(&self, b: Block) {
        if b.validate() {
            w(&self.chain).process_block(&b)
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

impl NetToChainAndPoolAdapter {
    pub fn init(&self, chain: Weak<Chain>) {
        let mut inner = self.chain.borrow_mut();
        *inner = Some(chain)
    }
}
