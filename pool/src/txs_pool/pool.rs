//! Top-level Pool type, methods, and tests
use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_notify::{Event, ForkBlocks, Notify, TXS_POOL_SUBSCRIBER};
use ckb_verification::{TransactionError, TransactionVerifier};
use core::block::Block;
use core::cell::{CellProvider, CellStatus};
use core::transaction::{OutPoint, ProposalShortId, Transaction};
use core::BlockNumber;
use crossbeam_channel;
use lru_cache::LruCache;
use std::sync::Arc;
use std::thread;
use txs_pool::types::*;
use util::{Mutex, RwLock};

/// The pool itself.
pub struct TransactionPool<T> {
    pub config: PoolConfig,
    /// The short id that has not been proposed
    pub pending: RwLock<PendingQueue>,
    /// The short id that has been proposed
    pub proposed: RwLock<ProposedQueue>,
    /// The  pool
    pub pool: RwLock<Pool>,
    /// Orphans in the pool
    pub orphan: RwLock<Orphan>,
    /// cache for conflict transaction
    pub cache: RwLock<LruCache<ProposalShortId, Transaction>>,
    /// chain will offer to the pool
    pub chain: Arc<T>,

    pub notify: Notify,

    lock: Arc<Mutex<usize>>,
}

impl<T> CellProvider for TransactionPool<T>
where
    T: ChainProvider,
{
    fn cell(&self, o: &OutPoint) -> CellStatus {
        match { self.pool.read().txo_status(o) } {
            TxoStatus::Spent => CellStatus::Old,
            TxoStatus::InPool => CellStatus::Current(self.pool.read().get_output(o).unwrap()),
            TxoStatus::Unknown => self.chain.cell(o),
        }
    }

    fn cell_at(&self, _o: &OutPoint, _parent: &H256) -> CellStatus {
        unreachable!()
    }
}

impl<T> TransactionPool<T>
where
    T: ChainProvider + 'static,
{
    /// Create a new transaction pool
    pub fn new(config: PoolConfig, chain: Arc<T>, notify: Notify) -> Arc<TransactionPool<T>> {
        let n = { chain.tip_header().read().number() };
        let cache_size = config.max_cache_size;
        let prop_cap = ProposedQueue::cap();
        let ids = chain.union_proposal_ids_n(n, prop_cap);

        let pool = Arc::new(TransactionPool {
            config,
            pending: RwLock::new(PendingQueue::new()),
            proposed: RwLock::new(ProposedQueue::new(n, ids)),
            pool: RwLock::new(Pool::new()),
            orphan: RwLock::new(Orphan::new()),
            cache: RwLock::new(LruCache::new(cache_size, false)),
            chain,
            notify,
            lock: Arc::new(Mutex::new(0_usize)),
        });

        let (tx, rx) = crossbeam_channel::unbounded();
        pool.notify
            .register_tip_subscriber(TXS_POOL_SUBSCRIBER, tx.clone());
        pool.notify
            .register_fork_subscriber(TXS_POOL_SUBSCRIBER, tx);
        let pool_cloned = Arc::<TransactionPool<T>>::clone(&pool);
        thread::spawn(move || loop {
            match rx.recv() {
                Some(Event::NewTip(b)) => {
                    pool_cloned.reconcile_block(&b);
                }
                Some(Event::SwitchFork(blks)) => {
                    pool_cloned.switch_fork(&blks);
                }
                None => {
                    info!(target: "txs_pool", "sub channel closed");
                    break;
                }
                event => {
                    warn!(target: "txs_pool", "Unexpected sub message {:?}", event);
                }
            }
        });

        pool
    }

    pub fn switch_fork(&self, blks: &ForkBlocks) {
        let _guard = self.lock.lock();
        {
            let mut pending = self.pending.write();
            let mut proposed = self.proposed.write();
            let mut orphan = self.orphan.write();
            let mut pool = self.pool.write();
            let mut cache = self.cache.write();

            for b in blks.old_blks() {
                let bn = b.header().number();
                let mut txs = b.commit_transactions().to_vec();
                txs.reverse();

                //remove proposed id, txs can be already in pool
                if let Some(rm_txs) = proposed.remove(bn) {
                    for (id, x) in rm_txs {
                        if let Some(tx) = x {
                            pending.insert(id, tx);
                        } else if let Some(txs) = pool.remove(&id) {
                            pending.insert(id, txs[0].clone());

                            for tx in txs.iter().skip(1) {
                                cache.insert(tx.proposal_short_id(), tx.clone());
                            }
                        } else if let Some(tx) = cache.remove(&id) {
                            pending.insert(id, tx);
                        } else if let Some(tx) = orphan.remove(&id) {
                            pending.insert(id, tx);
                        }
                    }
                }

                //readd txs in proposedqueue
                if let Some(frt_ids) = proposed.front().cloned() {
                    for id in frt_ids {
                        if let Some(txs) = pool.remove(&id) {
                            proposed.insert_without_check(id, txs[0].clone());
                            for tx in txs.iter().skip(1) {
                                cache.insert(tx.proposal_short_id(), tx.clone());
                            }
                        } else if let Some(tx) = cache.remove(&id) {
                            proposed.insert_without_check(id, tx);
                        } else if let Some(tx) = orphan.remove(&id) {
                            proposed.insert_without_check(id, tx);
                        }
                    }
                }

                //readd txs
                for tx in txs {
                    if tx.is_cellbase() {
                        continue;
                    }
                    pool.add_transaction(tx.clone());
                }
            }
            // We may not need readd timeout transactions in pool, because new main chain is mostly longer
        }

        for blk in blks.new_blks() {
            self.reconcile_block(&blk);
        }
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.pending.read().contains_key(id)
            || self.cache.read().contains_key(id)
            || self.pool.read().contains_key(id)
            || self.orphan.read().contains_key(id)
            || self.proposed.read().contains_key(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<Transaction> {
        self.pending
            .read()
            .get(id)
            .cloned()
            .or_else(|| self.proposed.read().get(id).cloned())
            .or_else(|| self.pool.read().get(id).cloned())
            .or_else(|| self.orphan.read().get(id).cloned())
            .or_else(|| self.cache.read().get(id).cloned())
    }

    /// Get the size of transactions in the pool
    pub fn pool_size(&self) -> usize {
        self.pool.read().size()
    }

    /// Get the size of orphans in the pool
    pub fn orphan_size(&self) -> usize {
        self.orphan.read().size()
    }

    /// Get the size of pending
    pub fn pending_size(&self) -> usize {
        self.pending.read().size()
    }

    /// Get the size of proposed
    pub fn proposed_size(&self) -> usize {
        self.proposed.read().size()
    }

    /// Get the size of cache
    pub fn cache_size(&self) -> usize {
        self.cache.read().len()
    }

    /// Get the total size (transactions + orphans) of the pool
    pub fn total_size(&self) -> usize {
        self.pool_size() + self.orphan_size()
    }

    pub fn add_transaction(&self, tx: Transaction) -> Result<InsertionResult, PoolError> {
        let _guard = self.lock.lock();
        match { self.proposed.write().insert(tx) } {
            TxStage::Mineable(x) => self.add_to_pool(x),
            TxStage::Unknown(x) => {
                self.pending.write().insert(x.proposal_short_id(), x);
                Ok(InsertionResult::Unknown)
            }
            _ => Ok(InsertionResult::Proposed),
        }
    }

    pub fn prepare_proposal(&self, n: usize) -> Vec<ProposalShortId> {
        let _guard = self.lock.lock();
        self.pending.read().fetch(n)
    }

    pub fn propose_transaction(&self, bn: BlockNumber, tx: Transaction) {
        let _guard = self.lock.lock();
        match { self.proposed.write().insert_with_n(bn, tx) } {
            TxStage::Mineable(x) => {
                let _ = self.add_to_pool(x);
            }
            TxStage::TimeOut(x) | TxStage::Fork(x) => {
                self.pending.write().insert(x.proposal_short_id(), x);
            }
            _ => {}
        };
    }

    pub fn get_mineable_transactions(&self, max: usize) -> Vec<Transaction> {
        let _guard = self.lock.lock();
        self.pool.read().get_mineable_transactions(max)
    }

    // Get all transactions that can be in next block, cache should added
    pub fn get_potential_transactions(&self) -> Vec<Transaction> {
        let _guard = self.lock.lock();
        let pool = self.pool.read();
        pool.get_mineable_transactions(pool.size())
    }

    /// Attempts to add a transaction to the memory pool.
    pub fn add_to_pool(&self, tx: Transaction) -> Result<InsertionResult, PoolError> {
        // Do we have the capacity to accept this transaction?
        self.is_acceptable()?;

        if tx.is_cellbase() {
            return Err(PoolError::CellBase);
        }

        self.check_duplicate(&tx)?;

        let inputs = tx.input_pts();
        let deps = tx.dep_pts();

        let mut unknowns = Vec::new();

        {
            let rtx = self.resolve_transaction(&tx);

            for (i, cs) in rtx.input_cells.iter().enumerate() {
                match cs {
                    CellStatus::Unknown => {
                        unknowns.push(inputs[i]);
                    }
                    CellStatus::Old => {
                        self.cache.write().insert(tx.proposal_short_id(), tx);
                        return Err(PoolError::DoubleSpent);
                    }
                    _ => {}
                }
            }

            for (i, cs) in rtx.dep_cells.iter().enumerate() {
                match cs {
                    CellStatus::Unknown => {
                        unknowns.push(deps[i]);
                    }
                    CellStatus::Old => {
                        self.cache.write().insert(tx.proposal_short_id(), tx);
                        return Err(PoolError::DoubleSpent);
                    }
                    _ => {}
                }
            }

            if unknowns.is_empty() {
                // TODO: Parallel
                TransactionVerifier::new(&rtx)
                    .verify()
                    .map_err(PoolError::InvalidTx)?;
            }
        }

        if !unknowns.is_empty() {
            self.orphan
                .write()
                .add_transaction(tx, unknowns.into_iter());
            return Ok(InsertionResult::Orphan);
        } else {
            {
                self.pool.write().add_transaction(tx.clone());
            }

            self.reconcile_orphan(&tx);

            self.notify.notify_new_transaction();
        }

        Ok(InsertionResult::Normal)
    }

    /// Updates the pool and orphan pool with new transactions.
    pub fn reconcile_orphan(&self, tx: &Transaction) {
        let txs = { self.orphan.write().reconcile_transaction(tx) };

        for tx in txs {
            let rtx = self.resolve_transaction(&tx);
            let rs = TransactionVerifier::new(&rtx).verify();
            if rs.is_ok() {
                self.pool.write().add_transaction(tx);
            } else if rs == Err(TransactionError::DoubleSpent) {
                self.cache.write().insert(tx.proposal_short_id(), tx);
            }
        }
    }

    /// Updates the pool with the details of a new block.
    // TODO: call it in order
    pub fn reconcile_block(&self, b: &Block) {
        let txs = b.commit_transactions();
        let bn = b.header().number();
        let ids = b.union_proposal_ids();

        // must do this first
        {
            for tx in txs {
                if tx.is_cellbase() {
                    continue;
                }

                self.reconcile_orphan(tx);
            }
        }

        // must do this secondly
        {
            let mut pool = self.pool.write();

            for tx in txs {
                if tx.is_cellbase() {
                    continue;
                }

                pool.commit_transaction(tx);
            }
        }

        {
            let mut pending = self.pending.write();
            let mut orphan = self.orphan.write();
            let mut pool = self.pool.write();

            if let Some(time_out_ids) = self.proposed.read().mineable_front() {
                for id in time_out_ids {
                    if let Some(txs) = pool.remove(id) {
                        for tx in txs {
                            pending.insert(tx.proposal_short_id(), tx);
                        }
                    } else if let Some(tx) = orphan.remove(id) {
                        pending.insert(tx.proposal_short_id(), tx);
                    }
                }
            }
        }

        let new_txs = {
            let mut pending = self.pending.write();
            let mut proposed = self.proposed.write();
            let mut cache = self.cache.write();

            for id in &ids {
                if let Some(tx) = pending.remove(id).or_else(|| cache.remove(id)) {
                    proposed.insert_without_check(id.clone(), tx);
                }
            }

            proposed.reconcile(bn, ids).unwrap()
        };

        // We can sort it by some rules
        for tx in new_txs {
            let _ = self.add_to_pool(tx);
        }
    }

    pub fn resolve_conflict(&self, tx: &Transaction) {
        if tx.is_cellbase() {
            return;
        }
        self.pool.write().resolve_conflict(tx);
    }

    /// Whether the pool is full
    fn is_acceptable(&self) -> Result<(), PoolError> {
        if self.total_size() > self.config.max_pool_size {
            // TODO evict old/large transactions instead
            return Err(PoolError::OverCapacity);
        }
        Ok(())
    }

    // Check that the transaction is not in the pool or chain
    fn check_duplicate(&self, tx: &Transaction) -> Result<(), PoolError> {
        let h = tx.hash();

        {
            if self.pool.read().contains(tx) || self.orphan.read().contains(tx) {
                return Err(PoolError::AlreadyInPool);
            }
        }

        if self.chain.contain_transaction(&h) {
            return Err(PoolError::DuplicateOutput);
        }

        Ok(())
    }
}
