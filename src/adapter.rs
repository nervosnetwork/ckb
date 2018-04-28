use chain::chain::Chain;
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
use network::{Broadcastable, Network};
use pool::{BlockChain, OrphanBlockPool, Parent, PendingBlockPool, PoolAdapter, TransactionPool};
use std::sync::Arc;
use std::sync::Weak;
use std::thread;
use time::{now_ms, Duration};
use util::{Mutex, RwLock};

type NetworkImpl = Network<NetToChainAndPoolAdapter>;
type ChainImpl = Chain<ChainToNetAndPoolAdapter, ChainKVStore<CacheKeyValueDB<RocksKeyValueDB>>>;
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
        self.tx_pool.reconcile_block(b);
        upgrade_network(&self.network).broadcast(Broadcastable::Block(box b.clone()));
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

#[derive(Clone)]
pub struct NetToChainAndPoolAdapter {
    key_group: Arc<KeyGroup>,
    orphan_pool: Arc<OrphanBlockPool>,
    pending_pool: Arc<PendingBlockPool>,
    tx_pool: Arc<TxPoolImpl>,
    chain: Weak<ChainImpl>,
    lock: Arc<Mutex<()>>,
}

impl NetAdapter for NetToChainAndPoolAdapter {
    fn block_received(&self, b: Block) {
        let _guard = self.lock.lock();
        if b.validate(&self.key_group).is_ok() {
            self.process_block(b);
        } else {
            // TODO ban remote peer
        }
    }

    fn transaction_received(&self, tx: Transaction) {
        let _guard = self.lock.lock();
        if let Err(e) = self.tx_pool.add_to_memory_pool(tx) {
            debug!("Transaction rejected: {:?}", e);
        }
    }
}

impl NetToChainAndPoolAdapter {
    pub fn new(
        kg: Arc<KeyGroup>,
        chain: &Arc<ChainImpl>,
        tx_pool: Arc<TxPoolImpl>,
        lock: Arc<Mutex<()>>,
    ) -> Arc<Self> {
        let adapter = Arc::new(NetToChainAndPoolAdapter {
            tx_pool,
            key_group: kg,
            orphan_pool: Arc::new(OrphanBlockPool::default()),
            pending_pool: Arc::new(PendingBlockPool::default()),
            chain: Arc::downgrade(chain),
            lock,
        });

        let subtask = Arc::clone(&adapter);
        thread::spawn(move || {
            let dur = Duration::from_millis(TIME_STEP);
            loop {
                thread::sleep(dur);
                subtask.handle_pending();
            }
        });

        adapter
    }

    pub fn is_orphan(&self, b: &Block) -> bool {
        upgrade_chain(&self.chain)
            .block_header(&b.header.pre_hash)
            .is_none()
    }

    pub fn process_block(&self, b: Block) {
        if b.header.timestamp > now_ms() {
            self.pending_pool.add_block(b);
        } else if self.is_orphan(&b) {
            if let Some(_h) = self.orphan_pool.add_block(b) {
                // TODO: self.request_block_by_hash(h)
            }
        } else {
            self.process_block_no_orphan(&b);
        }
    }

    pub fn process_block_no_orphan(&self, b: &Block) {
        if upgrade_chain(&self.chain).process_block(b).is_ok() {
            let blocks = self.orphan_pool.remove_block(&b.hash());

            for b in blocks {
                self.process_block_no_orphan(&b);
            }
        }
    }

    pub fn handle_pending(&self) {
        let blocks = self.pending_pool.get_block(now_ms());
        for b in blocks {
            self.process_block(b);
        }
    }
}

/// dependency between the pool and the chain.
pub struct PoolToChainAdapter {
    chain: ChainWeakRef,
}

impl PoolToChainAdapter {
    /// Create a new pool adapter
    pub fn new() -> PoolToChainAdapter {
        PoolToChainAdapter {
            chain: RwLock::new(None),
        }
    }

    /// Init
    pub fn init(&self, chain: &Arc<ChainImpl>) {
        let mut inner = self.chain.write();
        *inner = Some(Arc::downgrade(chain));
    }
}

impl BlockChain for PoolToChainAdapter {
    fn is_spent(&self, o: &OutPoint) -> Option<Parent> {
        match upgrade_chain_ref(&self.chain).cell(o) {
            CellState::Tail => Some(Parent::AlreadySpent),
            CellState::Head(_) => Some(Parent::BlockTransaction),
            CellState::Unknown => None,
        }
    }

    fn head_header(&self) -> Option<Header> {
        Some(upgrade_chain_ref(&self.chain).head_header())
    }
}

/// Adapter between the transaction pool and the network, to relay
/// transactions that have been accepted.
pub struct PoolToNetAdapter {
    network: NetworkWeakRef,
}

impl PoolAdapter for PoolToNetAdapter {
    fn tx_accepted(&self, _tx: &Transaction) {
        // brocast tx
    }
}

impl PoolToNetAdapter {
    /// Create a new pool to net adapter
    pub fn new() -> PoolToNetAdapter {
        PoolToNetAdapter {
            network: RwLock::new(None),
        }
    }

    /// init
    pub fn init(&self, network: &Arc<NetworkImpl>) {
        let mut inner = self.network.write();
        *inner = Some(Arc::downgrade(network));
    }
}
