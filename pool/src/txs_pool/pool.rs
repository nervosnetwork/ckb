//! Top-level Pool type, methods, and tests
use std::sync::Arc;

use core::block::Block;
use core::transaction::{OutPoint, Transaction};

use txs_pool::types::*;
use util::RwLock;

/// The pool itself.
pub struct TransactionPool {
    pub config: PoolConfig,
    /// The pool itself
    pub pool: RwLock<Pool>,
    /// Orphans in the pool
    pub orphan: RwLock<OrphanPool>,
    // chain will offer to the pool
    pub chain: Arc<BlockChain>,
    pub adapter: Arc<PoolAdapter>,
}

impl TransactionPool {
    /// Create a new transaction pool
    pub fn new(
        config: PoolConfig,
        chain: Arc<BlockChain>,
        adapter: Arc<PoolAdapter>,
    ) -> TransactionPool {
        TransactionPool {
            config,
            pool: RwLock::new(Pool::new()),
            orphan: RwLock::new(OrphanPool::new()),
            chain,
            adapter,
        }
    }

    pub fn is_spent(&self, o: &OutPoint) -> Parent {
        self.pool
            .read()
            .is_spent(o)
            .or_else(|| self.orphan.read().is_spent(o))
            .or_else(|| self.chain.is_spent(o))
            .unwrap_or(Parent::Unknown)
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

    /// Attempts to add a transaction to the stempool or the memory pool.
    pub fn add_to_memory_pool(&self, tx: Transaction) -> Result<(), PoolError> {
        // Do we have the capacity to accept this transaction?
        self.is_acceptable()?;

        // Making sure the transaction is valid before anything else.
        tx.validate(false).map_err(PoolError::InvalidTx)?;

        self.check_duplicate(&tx)?;

        let inputs = tx.input_pts();

        let mut is_orphan = false;
        let mut unknowns = Vec::new();

        for input in inputs {
            match self.is_spent(&input) {
                Parent::AlreadySpent => return Err(PoolError::DoubleSpend),
                Parent::OrphanTransaction => {
                    is_orphan = true;
                }
                Parent::Unknown => {
                    unknowns.push(input);
                    is_orphan = true;
                }
                _ => {}
            }
        }

        if is_orphan {
            self.adapter.tx_accepted(&tx);
            self.orphan.write().add_transaction(tx, unknowns);
            return Err(PoolError::OrphanTransaction);
        } else {
            self.adapter.tx_accepted(&tx);

            let txs = {
                let mut orphan = self.orphan.write();
                orphan.commit_transaction(&tx);
                orphan.get_no_orphan()
            };

            let mut pool = self.pool.write();

            pool.add_transaction(tx);

            for tx in txs {
                pool.add_transaction(tx);
            }
        }

        Ok(())
    }

    /// Updates the pool with the details of a new block.
    pub fn reconcile_block(&self, b: &Block) {
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

        let outputs = tx.output_pts();

        for o in outputs {
            if self.chain.is_spent(&o).is_some() {
                return Err(PoolError::DuplicateOutput);
            }
        }

        Ok(())
    }
}
