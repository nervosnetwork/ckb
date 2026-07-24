use crate::util::compact_packed;
use ckb_logger::debug;
use ckb_types::{
    core::{error::OutPointError, tx_pool::Reject},
    packed::{Byte32, OutPoint, ProposalShortId},
};
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
    #[cfg(test)]
    pub(crate) fn inputs_len(&self) -> usize {
        self.inputs.len()
    }

    #[cfg(test)]
    pub(crate) fn header_deps_len(&self) -> usize {
        self.header_deps.len()
    }

    #[cfg(test)]
    pub(crate) fn deps_len(&self) -> usize {
        self.deps.len()
    }

    pub(crate) fn insert_input(
        &mut self,
        out_point: OutPoint,
        txid: ProposalShortId,
    ) -> Result<(), Reject> {
        // The accessor may be a slice of the complete transaction. This key
        // can outlive that transaction when another indexed owner shares the
        // same outpoint, so make it independently owned before insertion.
        let out_point = compact_packed(&out_point);
        // inputs is occupied means double spending happened here
        match self.inputs.entry(out_point.clone()) {
            Entry::Occupied(occupied) => {
                debug!(
                    "txpool unexpected double-spending out_point: {:?} old_tx: {:?} new_tx: {:?}",
                    out_point,
                    occupied.get(),
                    txid
                );
                Err(Reject::Resolve(OutPointError::Dead(out_point)))
            }
            Entry::Vacant(vacant) => {
                vacant.insert(txid);
                Ok(())
            }
        }
    }

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

    pub(crate) fn clear(&mut self) {
        self.inputs.clear();
        self.deps.clear();
        self.header_deps.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::OutPointIndex;
    use ckb_types::{bytes::Bytes, packed::OutPoint, prelude::Entity};

    fn shared_out_point() -> (OutPoint, *const u8) {
        let template = OutPoint::default();
        let len = template.as_slice().len();
        let mut raw = vec![0x33; 4_096];
        raw[2_048..2_048 + len].copy_from_slice(template.as_slice());
        let backing = Bytes::from(raw);
        let out_point = OutPoint::new_unchecked(backing.slice(2_048..2_048 + len));
        let ptr = out_point.as_slice().as_ptr();
        (out_point, ptr)
    }

    #[test]
    fn persistent_outpoint_indexes_detach_shared_backing() {
        let mut index = OutPointIndex::default();
        let (input, input_ptr) = shared_out_point();
        index
            .insert_input(input, Default::default())
            .expect("vacant input");
        assert_ne!(
            index.inputs.keys().next().unwrap().as_slice().as_ptr(),
            input_ptr
        );

        let (dep, dep_ptr) = shared_out_point();
        index.insert_deps(dep, Default::default());
        assert_ne!(
            index.deps.keys().next().unwrap().as_slice().as_ptr(),
            dep_ptr
        );
    }
}
