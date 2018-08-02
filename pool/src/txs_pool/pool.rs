//! Top-level Pool type, methods, and tests
use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_notify::{Event, ForkTxs, Notify, TXS_POOL_SUBSCRIBER};
use ckb_verification::TransactionVerifier;
use core::block::IndexedBlock;
use core::cell::{CellProvider, CellState};
use core::transaction::{OutPoint, Transaction};
use crossbeam_channel;
use std::sync::Arc;
use std::thread;
use txs_pool::types::{InsertionResult, OrphanPool, Parent, Pool, PoolConfig, PoolError};
use util::RwLock;

/// The pool itself.
pub struct TransactionPool<T> {
    pub config: PoolConfig,
    /// The pool itself
    pub pool: RwLock<Pool>,
    /// Orphans in the pool
    pub orphan: RwLock<OrphanPool>,
    // chain will offer to the pool
    pub chain: Arc<T>,

    pub notify: Notify,
}

impl<T> CellProvider for TransactionPool<T>
where
    T: ChainProvider,
{
    fn cell(&self, o: &OutPoint) -> CellState {
        if self.pool.read().parent(o) == Parent::AlreadySpent
            || self.orphan.read().parent(o) == Parent::AlreadySpent
        {
            CellState::Tail
        } else if let Some(output) = self.pool.read().get_output(o) {
            CellState::Pool(output)
        } else if let Some(output) = self.orphan.read().get_output(o) {
            CellState::Orphan(output)
        } else {
            self.chain.cell(o)
        }
    }

    fn cell_at(&self, o: &OutPoint, parent: &H256) -> CellState {
        if self.pool.read().parent(o) == Parent::AlreadySpent
            || self.orphan.read().parent(o) == Parent::AlreadySpent
        {
            CellState::Tail
        } else if let Some(output) = self.pool.read().get_output(o) {
            CellState::Pool(output)
        } else if let Some(output) = self.orphan.read().get_output(o) {
            CellState::Orphan(output)
        } else {
            self.chain.cell_at(o, parent)
        }
    }
}

impl<T> TransactionPool<T>
where
    T: ChainProvider + 'static,
{
    /// Create a new transaction pool
    pub fn new(config: PoolConfig, chain: Arc<T>, notify: Notify) -> Arc<TransactionPool<T>> {
        let pool = Arc::new(TransactionPool {
            config,
            pool: RwLock::new(Pool::new()),
            orphan: RwLock::new(OrphanPool::new()),
            chain,
            notify,
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
                Some(Event::SwitchFork(txs)) => {
                    pool_cloned.switch_fork(&txs);
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

    pub fn switch_fork(&self, txs: &ForkTxs) {
        for tx in txs.old_txs() {
            self.pool.write().readd_transaction(&tx);
        }

        for tx in txs.new_txs().iter().rev() {
            let in_pool = { self.pool.write().commit_transaction(&tx) };
            if !in_pool {
                {
                    self.orphan.write().commit_transaction(&tx);
                }

                self.resolve_conflict(&tx);
            }
        }
    }

    /// Get the number of transactions in the pool
    pub fn pool_size(&self) -> usize {
        self.pool.read().size()
    }

    /// Get the number of orphans in the pool
    pub fn orphan_size(&self) -> usize {
        self.orphan.read().size()
    }

    /// Get the total size (transactions + orphans) of the pool
    pub fn total_size(&self) -> usize {
        self.pool_size() + self.orphan_size()
    }

    /// Attempts to add a transaction to the memory pool.
    pub fn add_to_memory_pool(&self, tx: Transaction) -> Result<InsertionResult, PoolError> {
        // Do we have the capacity to accept this transaction?
        self.is_acceptable()?;

        self.check_duplicate(&tx)?;

        let inputs = tx.input_pts();
        let deps = tx.dep_pts();

        let mut is_orphan = false;
        let mut unknowns = Vec::new();

        {
            let rtx = self.resolve_transaction(&tx);

            for (i, cs) in rtx.input_cells.iter().enumerate() {
                match cs {
                    CellState::Orphan(_) => is_orphan = true,
                    CellState::Unknown => {
                        is_orphan = true;
                        unknowns.push(inputs[i].clone());
                    }
                    _ => {}
                }
            }

            for (i, cs) in rtx.dep_cells.iter().enumerate() {
                match cs {
                    CellState::Orphan(_) => is_orphan = true,
                    CellState::Unknown => {
                        is_orphan = true;
                        unknowns.push(deps[i].clone());
                    }
                    _ => {}
                }
            }

            if unknowns.is_empty() {
                TransactionVerifier::new(rtx)
                    .verify()
                    .map_err(PoolError::InvalidTx)?;
            }
        }

        if is_orphan {
            self.orphan.write().add_transaction(tx, unknowns);
            return Ok(InsertionResult::Orphan);
        } else {
            let txs = {
                let mut orphan = self.orphan.write();
                orphan.commit_transaction(&tx);
                orphan.get_no_orphan()
            };

            let mut pool = self.pool.write();

            pool.add_transaction(tx);

            for tx in txs {
                let _ = self.add_to_memory_pool(tx);
            }

            self.notify.notify_new_transaction::<fn(&str) -> bool>(None);
        }

        Ok(InsertionResult::Normal)
    }

    /// Updates the pool with the details of a new block.
    pub fn reconcile_block(&self, b: &IndexedBlock) {
        let txs = &b.transactions;

        for tx in txs {
            let in_pool = { self.pool.write().commit_transaction(tx) };
            if !in_pool {
                {
                    self.orphan.write().commit_transaction(tx);
                }

                self.resolve_conflict(tx);
            }
        }
    }

    pub fn resolve_conflict(&self, tx: &Transaction) {
        self.pool.write().resolve_conflict(tx);
        self.orphan.write().resolve_conflict(tx);
    }

    /// Select a set of mineable transactions for block building.
    pub fn prepare_mineable_transactions(&self, n: usize) -> Vec<Transaction> {
        self.pool.read().get_mineable_transactions(n)
    }

    /// Whether the transaction is acceptable to the pool, given both how
    /// full the pool is and the transaction weight.
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
            if self.pool.read().is_pool_tx(&h) || self.orphan.read().is_pool_tx(&h) {
                return Err(PoolError::AlreadyInPool);
            }
        }

        if self.chain.contain_transaction(&h) {
            return Err(PoolError::DuplicateOutput);
        }

        Ok(())
    }
}
