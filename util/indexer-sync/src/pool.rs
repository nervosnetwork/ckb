//! A rebuildable view of inputs consumed by accepted pool transactions.

use ckb_tx_pool::{TxPoolController, TxPoolInputSnapshot};
use ckb_types::packed::{Byte32, OutPoint};
use std::{
    collections::HashSet,
    sync::{Arc, RwLock},
};

/// Inputs consumed by accepted transactions, shared by indexer queries.
#[derive(Default)]
pub struct Pool {
    dead_cells: Arc<HashSet<OutPoint>>,
}

impl From<Arc<HashSet<OutPoint>>> for Pool {
    fn from(dead_cells: Arc<HashSet<OutPoint>>) -> Self {
        Self { dead_cells }
    }
}

impl Pool {
    // Retain the previous inputs until this indexer has applied the pool's tip.
    // They bridge commitment in the pool to the corresponding database spend.
    fn update(&mut self, snapshot: TxPoolInputSnapshot, indexed_tip: &Byte32) {
        if snapshot.tip_hash == *indexed_tip {
            self.dead_cells = snapshot.inputs;
        }
    }

    /// Whether an accepted transaction consumes this outpoint.
    pub fn is_consumed_by_pool_tx(&self, out_point: &OutPoint) -> bool {
        self.dead_cells.contains(out_point)
    }

    /// All inputs consumed in this snapshot.
    pub fn dead_cells(&self) -> impl Iterator<Item = &OutPoint> {
        self.dead_cells.iter()
    }
}

/// Refreshes one indexer's pool view at the end of its block synchronization.
#[derive(Clone)]
pub struct PoolService {
    pool: Option<Arc<RwLock<Pool>>>,
    controller: TxPoolController,
}

impl PoolService {
    /// Create an optional pool view for the indexer synchronization loop.
    pub fn new(index_tx_pool: bool, controller: TxPoolController) -> Self {
        Self {
            pool: index_tx_pool.then(|| Arc::new(RwLock::new(Pool::default()))),
            controller,
        }
    }

    /// The last complete input snapshot published for this indexer.
    pub fn pool(&self) -> Option<Arc<RwLock<Pool>>> {
        self.pool.clone()
    }

    pub(crate) fn refresh(&self, indexed_tip: &Byte32) {
        let Some(pool) = &self.pool else {
            return;
        };
        match self.controller.input_snapshot() {
            Ok(snapshot) => {
                pool.write()
                    .expect("acquire lock")
                    .update(snapshot, indexed_tip);
            }
            Err(error) => ckb_logger::debug!("indexer pool snapshot unavailable: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_bridges_commit_until_indexer_catches_up() {
        let spent = OutPoint::default();
        let old_tip = Byte32::from([1; 32]);
        let new_tip = Byte32::from([2; 32]);
        let before = TxPoolInputSnapshot {
            tip_hash: old_tip.clone(),
            inputs: Arc::new(HashSet::from([spent.clone()])),
        };
        let mut pool = Pool::default();
        pool.update(before, &old_tip);
        let committed = TxPoolInputSnapshot {
            tip_hash: new_tip.clone(),
            inputs: Arc::new(HashSet::new()),
        };

        pool.update(committed.clone(), &old_tip);
        assert!(pool.is_consumed_by_pool_tx(&spent));
        pool.update(committed, &new_tip);
        assert!(!pool.is_consumed_by_pool_tx(&spent));
    }
}
