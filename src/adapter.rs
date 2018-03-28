use chain::chain::Chain;
use core::adapter::{ChainAdapter, NetAdapter};
use core::block::Block;
use core::cell::CellProvider;
use core::global::TIME_STEP;
use core::keygroup::KeyGroup;
use core::transaction::Transaction;
use db::cachedb::CacheKeyValueDB;
use db::diskdb::RocksKeyValueDB;
use db::store::ChainKVStore;
use network::{Broadcastable, Network};
use pool::{OrphanBlockPool, PendingBlockPool, TransactionPool};
use std::sync::Arc;
use std::sync::Weak;
use std::thread;
use time::{now_ms, Duration};
use util::{Mutex, RwLock};

type NetworkImpl = Network<NetToChainAndPoolAdapter>;
type ChainImpl = Chain<ChainToNetAndPoolAdapter, ChainKVStore<CacheKeyValueDB<RocksKeyValueDB>>>;
type NetworkWeakRef = RwLock<Option<Weak<NetworkImpl>>>;

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

pub struct ChainToNetAndPoolAdapter {
    tx_pool: Arc<TransactionPool>,
    network: NetworkWeakRef,
}

impl ChainAdapter for ChainToNetAndPoolAdapter {
    fn block_accepted(&self, b: &Block) {
        self.tx_pool.accommodate(b);
        upgrade_network(&self.network).broadcast(Broadcastable::Block(box b.clone()));
    }
}

impl ChainToNetAndPoolAdapter {
    pub fn new(tx_pool: Arc<TransactionPool>) -> Self {
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
    tx_pool: Arc<TransactionPool>,
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

    // TODO: the logic should not be in adapter
    fn transaction_received(&self, tx: Transaction) {
        let _guard = self.lock.lock();
        if tx.validate(false) {
            let mut resolved_tx = upgrade_chain(&self.chain).resolve_transaction(tx);
            if resolved_tx.is_double_spend() {
                debug!(target: "tx", "tx double spends");
                // TODO process double spend tx
                return;
            }
            self.tx_pool
                .resolve_transaction_unknown_inputs(&mut resolved_tx);
            if resolved_tx.is_double_spend() {
                debug!(target: "tx", "tx double spends");
                // TODO process double spend tx
                return;
            }
            if resolved_tx.is_orphan() {
                // TODO add to orphan pool
                debug!(target: "tx", "tx is orphan");
            } else if resolved_tx.validate(false) {
                // TODO check orphan pool
                self.tx_pool.add_transaction(resolved_tx.transaction);
                debug!(target: "tx", "tx is added to pool");
            } else {
                // TODO ban remote peer
                debug!(target: "tx", "tx is invalid with resolved inputs");
            }
        } else {
            // TODO ban remote peer
            debug!(target: "tx", "tx is invalid");
        }
    }
}

impl NetToChainAndPoolAdapter {
    pub fn new(
        kg: Arc<KeyGroup>,
        chain: &Arc<ChainImpl>,
        tx_pool: Arc<TransactionPool>,
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
