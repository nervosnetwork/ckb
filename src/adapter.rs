use chain::chain::Chain;
use core::adapter::{ChainAdapter, NetAdapter};
use core::block::Block;
use core::global::TIME_STEP;
use core::keygroup::KeyGroup;
use core::transaction::Transaction;
use network::Network;
use pool::{OrphanBlockPool, PendingBlockPool, TransactionPool};
use std::sync::Arc;
use std::thread;
use time::{now_ms, Duration};
use util::RwLock;

pub struct ChainToNetAndPoolAdapter {
    tx_pool: Arc<TransactionPool>,
    network: RwLock<Arc<Network>>,
}

impl ChainAdapter for ChainToNetAndPoolAdapter {
    fn block_accepted(&self, b: &Block) {
        self.tx_pool.accommodate(b);
        self.network.read().broadcast(b)
    }
}

impl ChainToNetAndPoolAdapter {
    pub fn new(tx_pool: Arc<TransactionPool>) -> Self {
        ChainToNetAndPoolAdapter {
            tx_pool: tx_pool,
            network: RwLock::new(Arc::new(
                Network::init(Arc::new(FakeNet::default()), vec![], vec![]).unwrap(),
            )),
        }
    }
    pub fn init(&self, network: Arc<Network>) {
        let mut inner = self.network.write();
        *inner = network;
    }
}

#[derive(Clone, Default)]
pub struct FakeNet {}

impl NetAdapter for FakeNet {
    fn block_received(&self, _: Block) {
        unimplemented!();
    }

    fn transaction_received(&self, _: Transaction) {
        unimplemented!();
    }
}

#[derive(Clone)]
pub struct NetToChainAndPoolAdapter {
    key_group: Arc<KeyGroup>,
    orphan_pool: Arc<OrphanBlockPool>,
    pending_pool: Arc<PendingBlockPool>,
    tx_pool: Arc<TransactionPool>,
    chain: Arc<Chain>,
}

impl NetAdapter for NetToChainAndPoolAdapter {
    fn block_received(&self, b: Block) {
        if b.validate(&self.key_group).is_ok() {
            self.process_block(b);
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
    pub fn new(kg: Arc<KeyGroup>, chain: Arc<Chain>, tx_pool: Arc<TransactionPool>) -> Arc<Self> {
        let adapter = Arc::new(NetToChainAndPoolAdapter {
            key_group: kg,
            orphan_pool: Arc::new(OrphanBlockPool::default()),
            pending_pool: Arc::new(PendingBlockPool::default()),
            tx_pool: tx_pool,
            chain: chain,
        });

        let subtask = adapter.clone();
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
        self.chain.block_header(&b.header.pre_hash).is_none()
    }

    pub fn process_block(&self, b: Block) {
        if b.header.timestamp > now_ms() {
            self.pending_pool.add_block(b);
        } else if self.is_orphan(&b) {
            if self.orphan_pool.add_block(b) {
                // TODO: request pre block
            }
        } else {
            self.process_block_no_orphan(&b);
        }
    }

    pub fn process_block_no_orphan(&self, b: &Block) {
        if self.chain.process_block(b).is_ok() {
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
