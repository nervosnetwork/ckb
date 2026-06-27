//! Tracks output out-points that are still in flight inside the tx-pool pipeline.
//!
//! When a transaction is sitting in one of the pipeline queues its outputs are
//! not yet available in the chain snapshot.  By indexing those outputs we can
//! quickly detect newly arrived transactions that depend on in-flight txs and
//! route them directly to the ordered resolver, avoiding a failing pre-resolve
//! and the resulting orphan-pool churn.

use ckb_types::core::TransactionView;
use ckb_types::packed::{OutPoint, ProposalShortId};
use std::collections::HashMap;

/// Maps an output out-point to the short id of the transaction that produces it.
///
/// Maintains a reverse index (`ProposalShortId` → out-points) so that
/// [`remove`](Self::remove) runs in O(outputs-per-tx) instead of O(total-entries).
#[derive(Default)]
pub(crate) struct FlightTracker {
    out_points: HashMap<OutPoint, ProposalShortId>,
    reverse: HashMap<ProposalShortId, Vec<OutPoint>>,
}

impl FlightTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self {
            out_points: HashMap::new(),
            reverse: HashMap::new(),
        }
    }

    /// Register the outputs of a transaction.
    pub fn insert(&mut self, id: ProposalShortId, tx: &TransactionView) {
        let out_pts: Vec<OutPoint> = (0..tx.outputs().len())
            .map(|index| OutPoint::new(tx.hash(), index as u32))
            .collect();
        for out_point in &out_pts {
            self.out_points.insert(out_point.clone(), id.clone());
        }
        self.reverse.insert(id, out_pts);
    }

    /// Remove all outputs belonging to the given transaction.
    pub fn remove(&mut self, id: &ProposalShortId) {
        if let Some(out_pts) = self.reverse.remove(id) {
            for out_point in out_pts {
                self.out_points.remove(&out_point);
            }
        }
    }

    /// Returns true if any input or cell dep of `tx` is produced by a
    /// transaction currently tracked by this tracker.
    pub fn depends_on(&self, tx: &TransactionView) -> bool {
        if self.out_points.is_empty() {
            return false;
        }
        tx.input_pts_iter()
            .chain(tx.cell_deps_iter().map(|c| c.out_point()))
            .any(|out_point| self.out_points.contains_key(&out_point))
    }

    /// Clear all tracked outputs.
    pub fn clear(&mut self) {
        self.out_points.clear();
        self.reverse.clear();
    }
}
