use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use std::collections::{hash_map::Entry, HashMap, HashSet};

#[derive(Debug, PartialEq, Clone)]
pub(crate) enum OutPointStatus {
    UnConsumed,
    Consumed(ProposalShortId),
}

#[derive(Default, Debug, Clone)]
pub(crate) struct Edges {
    /// input-txid map represent in-pool tx's inputs
    pub(crate) inputs: HashMap<OutPoint, ProposalShortId>,
    /// output-op<txid> map represent in-pool tx's outputs
    pub(crate) outputs: HashMap<OutPoint, OutPointStatus>,
    /// dep-set<txid> map represent in-pool tx's deps
    pub(crate) deps: HashMap<OutPoint, HashSet<ProposalShortId>>,
    /// dep-set<txid-headers> map represent in-pool tx's header deps
    pub(crate) header_deps: HashMap<ProposalShortId, Vec<Byte32>>,
}

impl Edges {
    #[cfg(test)]
    pub(crate) fn outputs_len(&self) -> usize {
        self.outputs.len()
    }

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

    pub(crate) fn insert_input(&mut self, out_point: OutPoint, txid: ProposalShortId) {
        self.inputs.insert(out_point, txid);
    }

    pub(crate) fn remove_input(&mut self, out_point: &OutPoint) -> Option<ProposalShortId> {
        self.inputs.remove(out_point)
    }

    pub(crate) fn remove_output(&mut self, out_point: &OutPoint) -> Option<ProposalShortId> {
        match self.outputs.remove(out_point) {
            Some(OutPointStatus::Consumed(id)) => Some(id),
            _ => None,
        }
    }

    pub(crate) fn insert_unconsumed_output(&mut self, out_point: OutPoint) {
        self.outputs.insert(out_point, OutPointStatus::UnConsumed);
    }

    pub(crate) fn insert_consumed_output(&mut self, out_point: OutPoint, id: ProposalShortId) {
        self.outputs.insert(out_point, OutPointStatus::Consumed(id));
    }

    pub(crate) fn get_input_ref(&self, out_point: &OutPoint) -> Option<&ProposalShortId> {
        self.inputs.get(out_point)
    }

    pub(crate) fn get_deps_ref(&self, out_point: &OutPoint) -> Option<&HashSet<ProposalShortId>> {
        self.deps.get(out_point)
    }

    pub(crate) fn set_output_consumed(
        &mut self,
        out_point: &OutPoint,
        tx_short_id: &ProposalShortId,
    ) {
        if let Some(status) = self.outputs.get_mut(out_point) {
            *status = OutPointStatus::Consumed(tx_short_id.clone());
        }
    }

    pub(crate) fn set_output_unconsumed(&mut self, out_point: &OutPoint) {
        if let Some(status) = self.outputs.get_mut(out_point) {
            *status = OutPointStatus::UnConsumed;
        }
    }

    pub(crate) fn get_output_ref(&self, out_point: &OutPoint) -> Option<&OutPointStatus> {
        self.outputs.get(out_point)
    }

    pub(crate) fn insert_deps(&mut self, out_point: OutPoint, txid: ProposalShortId) {
        self.deps.entry(out_point).or_default().insert(txid);
    }

    pub(crate) fn delete_txid_by_dep(&mut self, out_point: OutPoint, txid: &ProposalShortId) {
        if let Entry::Occupied(mut occupied) = self.deps.entry(out_point) {
            let ids = occupied.get_mut();
            ids.remove(txid);
            if ids.is_empty() {
                occupied.remove();
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.inputs.clear();
        self.outputs.clear();
        self.deps.clear();
        self.header_deps.clear();
    }
}
