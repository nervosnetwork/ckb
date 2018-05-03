use chain::chain::Chain;
use chain::chain::ChainClient;
use core::adapter::{ChainAdapter, NetAdapter};
use core::block::Block;
use core::block::Header;
use core::cell::{CellProvider, CellState};
use core::global::TIME_STEP;
use core::keygroup::KeyGroup;
use core::transaction::{OutPoint, Transaction};
use db::cachedb::CacheKeyValueDB;
use db::diskdb::RocksKeyValueDB;
use db::store::ChainKVStore;
use network::Network;
use pool::{OrphanBlockPool, PendingBlockPool, TransactionPool};
use std::sync::Arc;
use std::sync::Weak;
use std::thread;
use time::{now_ms, Duration};
use util::{Mutex, RwLock};

type NetworkImpl = Network;
type ChainImpl = Chain<ChainKVStore<CacheKeyValueDB<RocksKeyValueDB>>>;
type NetworkWeakRef = RwLock<Option<Weak<NetworkImpl>>>;
type ChainWeakRef = RwLock<Option<Weak<ChainImpl>>>;
type TxPoolImpl = TransactionPool<PoolToChainAdapter>;

fn upgrade_chain(chain: &Weak<ChainImpl>) -> Arc<ChainImpl> {
    chain.upgrade().expect("Chain must haven't dropped.")
}

fn upgrade_network(network: &NetworkWeakRef) -> Arc<NetworkImpl> {
    network
        .read()
        .as_ref()
        .and_then(|weak| weak.upgrade())
        .expect("ChainAdapter methods are called after network is init.")
}

fn upgrade_chain_ref(chain: &ChainWeakRef) -> Arc<ChainImpl> {
    chain
        .read()
        .as_ref()
        .and_then(|weak| weak.upgrade())
        .expect("Chain is not init.")
}

pub struct ChainToNetAndPoolAdapter {
    tx_pool: Arc<TxPoolImpl>,
    network: NetworkWeakRef,
}

impl ChainAdapter for ChainToNetAndPoolAdapter {
    fn block_accepted(&self, b: &Block) {
        self.tx_pool.accommodate(b);
        upgrade_network(&self.network).broadcast(vec![1, 2, 3, 4]);
    }
}

impl ChainToNetAndPoolAdapter {
    pub fn new(tx_pool: Arc<TxPoolImpl>) -> Self {
        ChainToNetAndPoolAdapter {
            tx_pool,
            network: RwLock::new(None),
        }
    }
    pub fn init(&self, network: &Arc<NetworkImpl>) {
        let mut inner = self.network.write();
        *inner = Some(Arc::downgrade(network));
    }
}
