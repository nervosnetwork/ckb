use crate::util::compact_packed;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use std::collections::{HashMap, HashSet, hash_map::Entry};

/// Index that maps consumed or referenced out-points to the in-pool transactions
/// that depend on them.
///
/// `inputs` records which transaction currently spends a given cell (used for
/// double-spend detection). `deps` records which transactions reference a cell
/// as a cell dep. `header_deps` records header dependencies for in-pool txs.
/// This structure was historically called `Edges`; the current name reflects its
/// actual purpose as an out-point → transaction index.
#[derive(Default, Debug, Clone)]
pub(crate) struct OutPointIndex {
    /// input-txid map represent in-pool tx's inputs
    pub(crate) inputs: HashMap<OutPoint, ProposalShortId>,
    /// dep-set<txid> map represent in-pool tx's deps
    pub(crate) deps: HashMap<OutPoint, HashSet<ProposalShortId>>,
    /// dep-set<txid-headers> map represent in-pool tx's header deps
    pub(crate) header_deps: HashMap<ProposalShortId, Vec<Byte32>>,
}

impl OutPointIndex {
    pub(crate) fn remove_input(&mut self, out_point: &OutPoint) -> Option<ProposalShortId> {
        self.inputs.remove(out_point)
    }

    pub(crate) fn get_input_ref(&self, out_point: &OutPoint) -> Option<&ProposalShortId> {
        self.inputs.get(out_point)
    }

    pub(crate) fn get_deps_ref(&self, out_point: &OutPoint) -> Option<&HashSet<ProposalShortId>> {
        self.deps.get(out_point)
    }

    pub(crate) fn insert_deps(&mut self, out_point: OutPoint, txid: ProposalShortId) {
        let out_point = compact_packed(&out_point);
        self.deps.entry(out_point).or_default().insert(txid);
    }

    pub(crate) fn delete_txid_by_dep(
        &mut self,
        out_point: OutPoint,
        txid: &ProposalShortId,
    ) -> bool {
        if let Entry::Occupied(mut occupied) = self.deps.entry(out_point) {
            let ids = occupied.get_mut();
            let removed = ids.remove(txid);
            if ids.is_empty() {
                occupied.remove();
            }
            removed
        } else {
            false
        }
    }
}

#[cfg(test)]
#[path = "tests/out_point_index.rs"]
mod tests;
