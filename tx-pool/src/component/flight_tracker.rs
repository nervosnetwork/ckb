//! Tracks output and input out-points that are still in flight inside the
//! tx-pool pipeline.
//!
//! When a transaction is sitting in one of the pipeline queues its outputs are
//! not yet available in the chain snapshot.  By indexing those outputs we can
//! quickly detect newly arrived transactions that depend on in-flight txs and
//! route them directly to the ordered resolver, avoiding a failing pre-resolve
//! and the resulting orphan-pool churn.
//!
//! In addition to outputs, inputs are also tracked.  If a transaction in flight
//! is going to consume an on-chain cell, any later transaction that references
//! the same cell (either as an input or as a cell dep) must wait until the
//! earlier transaction resolves; otherwise the later transaction could observe
//! a stale `CellStatus::Live` and race with the earlier transaction.

use ckb_types::core::TransactionView;
use ckb_types::packed::{OutPoint, ProposalShortId};
use std::collections::HashMap;

/// Maps in-flight output and input out-points to the short id of the
/// transaction that holds them.
///
/// Outputs and inputs are kept in separate maps because a child's input is the
/// parent's output: if removing the child erased that shared out-point, the
/// parent would no longer be tracked and later dependents would not be ordered
/// after it.
///
/// Maintains reverse indexes (`ProposalShortId` → out-points) so that
/// [`remove`](Self::remove) runs in O(outputs+inputs-per-tx) instead of
/// O(total-entries).
#[derive(Default)]
pub(crate) struct FlightTracker {
    outputs: HashMap<OutPoint, ProposalShortId>,
    inputs: HashMap<OutPoint, ProposalShortId>,
    reverse_outputs: HashMap<ProposalShortId, Vec<OutPoint>>,
    reverse_inputs: HashMap<ProposalShortId, Vec<OutPoint>>,
}

impl FlightTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self {
            outputs: HashMap::new(),
            inputs: HashMap::new(),
            reverse_outputs: HashMap::new(),
            reverse_inputs: HashMap::new(),
        }
    }

    /// Register the outputs and inputs of a transaction.
    pub fn insert(&mut self, id: ProposalShortId, tx: &TransactionView) {
        let output_pts: Vec<OutPoint> = (0..tx.outputs().len())
            .map(|index| OutPoint::new(tx.hash(), index as u32))
            .collect();
        let input_pts: Vec<OutPoint> = tx.input_pts_iter().collect();

        for out_point in &output_pts {
            self.outputs.insert(out_point.clone(), id.clone());
        }
        for out_point in &input_pts {
            self.inputs.insert(out_point.clone(), id.clone());
        }
        self.reverse_outputs.insert(id.clone(), output_pts);
        self.reverse_inputs.insert(id, input_pts);
    }

    /// Remove all out-points belonging to the given transaction.
    pub fn remove(&mut self, id: &ProposalShortId) {
        if let Some(pts) = self.reverse_outputs.remove(id) {
            for out_point in pts {
                if self.outputs.get(&out_point) == Some(id) {
                    self.outputs.remove(&out_point);
                }
            }
        }
        if let Some(pts) = self.reverse_inputs.remove(id) {
            for out_point in pts {
                if self.inputs.get(&out_point) == Some(id) {
                    self.inputs.remove(&out_point);
                }
            }
        }
    }

    /// Returns true if any input or cell dep of `tx` is referenced by a
    /// transaction currently tracked by this tracker.
    pub fn depends_on(&self, tx: &TransactionView) -> bool {
        if self.outputs.is_empty() && self.inputs.is_empty() {
            return false;
        }
        tx.input_pts_iter()
            .chain(tx.cell_deps_iter().map(|c| c.out_point()))
            .any(|out_point| {
                self.outputs.contains_key(&out_point) || self.inputs.contains_key(&out_point)
            })
    }

    /// Clear all tracked out-points.
    pub fn clear(&mut self) {
        self.outputs.clear();
        self.inputs.clear();
        self.reverse_outputs.clear();
        self.reverse_inputs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_types::{
        bytes::Bytes,
        core::{Capacity, TransactionBuilder},
        packed::{CellDep, CellInput, CellOutput, Script},
        prelude::*,
    };

    fn mock_input_out_point() -> OutPoint {
        OutPoint::new(
            ckb_types::h256!("0x0101010101010101010101010101010101010101010101010101010101010101")
                .pack(),
            0,
        )
    }

    fn mock_other_input_out_point() -> OutPoint {
        OutPoint::new(
            ckb_types::h256!("0x0202020202020202020202020202020202020202020202020202020202020202")
                .pack(),
            0,
        )
    }

    fn mock_output_out_point(tx: &TransactionView, index: u32) -> OutPoint {
        OutPoint::new(tx.hash(), index)
    }

    fn build_tx(input: &OutPoint, output_capacity: u64) -> TransactionView {
        let output = CellOutput::new_builder()
            .capacity(Capacity::bytes(output_capacity as usize).unwrap())
            .lock(Script::default())
            .build();
        TransactionBuilder::default()
            .input(CellInput::new(input.clone(), 0))
            .output(output)
            .output_data(Bytes::default().pack())
            .build()
    }

    fn build_tx_with_cell_dep(input: &OutPoint, cell_dep: &OutPoint) -> TransactionView {
        let output = CellOutput::new_builder()
            .capacity(Capacity::bytes(1_000).unwrap())
            .lock(Script::default())
            .build();
        TransactionBuilder::default()
            .input(CellInput::new(input.clone(), 0))
            .cell_dep(CellDep::new_builder().out_point(cell_dep.clone()).build())
            .output(output)
            .output_data(Bytes::default().pack())
            .build()
    }

    #[test]
    fn tracks_outputs() {
        let mut tracker = FlightTracker::new();
        let input = mock_input_out_point();
        let tx = build_tx(&input, 1_000);
        let id = tx.proposal_short_id();
        tracker.insert(id.clone(), &tx);

        // A child spending the tx's output should depend on it.
        let child_input = mock_output_out_point(&tx, 0);
        let child = build_tx(&child_input, 500);
        assert!(tracker.depends_on(&child));

        // An unrelated tx should not.
        let unrelated = build_tx(&mock_other_input_out_point(), 500);
        assert!(!tracker.depends_on(&unrelated));

        tracker.remove(&id);
        assert!(!tracker.depends_on(&child));
    }

    #[test]
    fn tracks_inputs() {
        let mut tracker = FlightTracker::new();
        let on_chain_cell = mock_input_out_point();
        let tx = build_tx(&on_chain_cell, 1_000);
        let id = tx.proposal_short_id();
        tracker.insert(id.clone(), &tx);

        // Another tx trying to spend the same on-chain cell should be detected.
        let double_spend = build_tx(&on_chain_cell, 500);
        assert!(tracker.depends_on(&double_spend));

        // A tx using the same cell as a cell dep should also be detected.
        let other_input = mock_other_input_out_point();
        let cell_dep_user = build_tx_with_cell_dep(&other_input, &on_chain_cell);
        assert!(tracker.depends_on(&cell_dep_user));

        tracker.remove(&id);
        assert!(!tracker.depends_on(&double_spend));
        assert!(!tracker.depends_on(&cell_dep_user));
    }

    #[test]
    fn clear_removes_everything() {
        let mut tracker = FlightTracker::new();
        let input = mock_input_out_point();
        let tx = build_tx(&input, 1_000);
        tracker.insert(tx.proposal_short_id(), &tx);

        let double_spend = build_tx(&input, 500);
        assert!(tracker.depends_on(&double_spend));

        tracker.clear();
        assert!(!tracker.depends_on(&double_spend));
    }
}
