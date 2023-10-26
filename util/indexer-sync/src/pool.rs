//! An overlay to index the pending txs in the ckb tx pool

use ckb_types::{core::TransactionView, packed::OutPoint};
use std::collections::HashSet;

/// An overlay to index the pending txs in the ckb tx pool,
/// currently only supports removals of dead cells from the pending txs
#[derive(Default)]
pub struct Pool {
    dead_cells: HashSet<OutPoint>,
}

impl Pool {
    /// the tx has been committed in a block, it should be removed from pending dead cells
    pub fn transaction_committed(&mut self, tx: &TransactionView) {
        for input in tx.inputs() {
            self.dead_cells.remove(&input.previous_output());
        }
    }

    /// the tx has been rejected for some reason, it should be removed from pending dead cells
    pub fn transaction_rejected(&mut self, tx: &TransactionView) {
        for input in tx.inputs() {
            self.dead_cells.remove(&input.previous_output());
        }
    }

    /// a new tx is submitted to the pool, mark its inputs as dead cells
    pub fn new_transaction(&mut self, tx: &TransactionView) {
        for input in tx.inputs() {
            self.dead_cells.insert(input.previous_output());
        }
    }

    /// Return wether out_point referred cell consumed by pooled transaction
    pub fn is_consumed_by_pool_tx(&self, out_point: &OutPoint) -> bool {
        self.dead_cells.contains(out_point)
    }

    /// the txs has been committed in a block, it should be removed from pending dead cells
    pub fn transactions_committed(&mut self, txs: &[TransactionView]) {
        for tx in txs {
            self.transaction_committed(tx);
        }
    }
}
