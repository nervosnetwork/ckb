use crate::component::entry::TxEntry;
use ckb_types::{
    core::{
        cell::{CellChecker, CellMetaBuilder, CellProvider, CellStatus},
        error::OutPointError,
        tx_pool::Reject,
        TransactionView,
    },
    packed::{Byte32, OutPoint, ProposalShortId},
    prelude::*,
};
use ckb_util::{LinkedHashMap, LinkedHashMapEntries};
use std::collections::{hash_map::Entry, HashMap, HashSet, VecDeque};

type ConflictEntry = (TxEntry, Reject);

#[derive(Debug, Clone)]
pub(crate) struct PendingQueue {
    pub(crate) inner: LinkedHashMap<ProposalShortId, TxEntry>,
    /// dep-set<txid> map represent in-pool tx's deps
    pub(crate) deps: HashMap<OutPoint, HashSet<ProposalShortId>>,
    /// input-set<txid> map represent in-pool tx's inputs
    pub(crate) inputs: HashMap<OutPoint, HashSet<ProposalShortId>>,
    /// dep-set<txid-headers> map represent in-pool tx's header deps
    pub(crate) header_deps: HashMap<ProposalShortId, Vec<Byte32>>,
    // /// output-op<txid> map represent in-pool tx's outputs
    pub(crate) outputs: HashMap<OutPoint, HashSet<ProposalShortId>>,
}

impl PendingQueue {
    pub(crate) fn new() -> Self {
        PendingQueue {
            inner: Default::default(),
            deps: Default::default(),
            inputs: Default::default(),
            header_deps: Default::default(),
            outputs: Default::default(),
        }
    }

    pub(crate) fn size(&self) -> usize {
        self.inner.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }

    #[cfg(test)]
    pub(crate) fn outputs_len(&self) -> usize {
        self.outputs.len()
    }

    #[cfg(test)]
    pub(crate) fn header_deps_len(&self) -> usize {
        self.header_deps.len()
    }

    #[cfg(test)]
    pub(crate) fn deps_len(&self) -> usize {
        self.deps.len()
    }

    #[cfg(test)]
    pub(crate) fn inputs_len(&self) -> usize {
        self.inputs.len()
    }

    pub(crate) fn add_entry(&mut self, entry: TxEntry) -> bool {
        let tx_short_id = entry.proposal_short_id();
        if self.inner.contains_key(&tx_short_id) {
            return false;
        }

        let inputs = entry.transaction().input_pts_iter();
        let outputs = entry.transaction().output_pts();

        for i in inputs {
            self.inputs
                .entry(i.to_owned())
                .or_default()
                .insert(tx_short_id.clone());

            if let Some(outputs) = self.outputs.get_mut(&i) {
                outputs.insert(tx_short_id.clone());
            }
        }

        // record dep-txid
        for d in entry.related_dep_out_points() {
            self.deps
                .entry(d.to_owned())
                .or_default()
                .insert(tx_short_id.clone());

            if let Some(outputs) = self.outputs.get_mut(d) {
                outputs.insert(tx_short_id.clone());
            }
        }

        // record tx unconsumed output
        for o in outputs {
            self.outputs.insert(o, HashSet::new());
        }

        // record header_deps
        let header_deps = entry.transaction().header_deps();
        if !header_deps.is_empty() {
            self.header_deps
                .insert(tx_short_id.clone(), header_deps.into_iter().collect());
        }

        self.inner.insert(tx_short_id, entry);
        true
    }

    pub(crate) fn resolve_conflict(&mut self, tx: &TransactionView) -> Vec<ConflictEntry> {
        let inputs = tx.input_pts_iter();
        let mut conflicts = Vec::new();

        for i in inputs {
            if let Some(ids) = self.inputs.remove(&i) {
                for id in ids {
                    let entries = self.remove_entry_and_descendants(&id);
                    for entry in entries {
                        let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                        conflicts.push((entry, reject));
                    }
                }
            }

            // deps consumed
            if let Some(ids) = self.deps.remove(&i) {
                for id in ids {
                    let entries = self.remove_entry_and_descendants(&id);
                    for entry in entries {
                        let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                        conflicts.push((entry, reject));
                    }
                }
            }
        }
        conflicts
    }

    pub(crate) fn resolve_conflict_header_dep(
        &mut self,
        headers: &HashSet<Byte32>,
    ) -> Vec<ConflictEntry> {
        let mut conflicts = Vec::new();

        // invalid header deps
        let mut ids = Vec::new();
        for (tx_id, deps) in self.header_deps.iter() {
            for hash in deps {
                if headers.contains(hash) {
                    ids.push((hash.clone(), tx_id.clone()));
                    break;
                }
            }
        }

        for (blk_hash, id) in ids {
            let entries = self.remove_entry_and_descendants(&id);
            for entry in entries {
                let reject = Reject::Resolve(OutPointError::InvalidHeader(blk_hash.to_owned()));
                conflicts.push((entry, reject));
            }
        }
        conflicts
    }

    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.contains_key(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ProposalShortId, &TxEntry)> {
        self.inner.iter()
    }

    pub(crate) fn get(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.inner.get(id)
    }

    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.inner.get(id).map(|entry| entry.transaction())
    }

    pub(crate) fn remove_entry(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        let removed = self.inner.remove(id);

        if let Some(ref entry) = removed {
            self.remove_entry_relation(entry);
        }

        removed
    }

    pub(crate) fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        let mut removed = Vec::new();
        if let Some(entry) = self.inner.remove(id) {
            let descendants = self.get_descendants(&entry);
            self.remove_entry_relation(&entry);
            removed.push(entry);
            for id in descendants {
                if let Some(entry) = self.remove_entry(&id) {
                    removed.push(entry);
                }
            }
        }
        removed
    }

    pub(crate) fn get_descendants(&self, entry: &TxEntry) -> HashSet<ProposalShortId> {
        let mut entries: VecDeque<&TxEntry> = VecDeque::new();
        entries.push_back(entry);

        let mut descendants = HashSet::new();
        while let Some(entry) = entries.pop_front() {
            let outputs = entry.transaction().output_pts();

            for output in outputs {
                if let Some(ids) = self.outputs.get(&output) {
                    for id in ids {
                        if descendants.insert(id.clone()) {
                            if let Some(entry) = self.inner.get(id) {
                                entries.push_back(entry);
                            }
                        }
                    }
                }
            }
        }
        descendants
    }

    pub(crate) fn remove_entry_relation(&mut self, entry: &TxEntry) {
        let inputs = entry.transaction().input_pts_iter();
        let tx_short_id = entry.proposal_short_id();
        let outputs = entry.transaction().output_pts();

        for i in inputs {
            if let Entry::Occupied(mut occupied) = self.inputs.entry(i) {
                let empty = {
                    let ids = occupied.get_mut();
                    ids.remove(&tx_short_id);
                    ids.is_empty()
                };
                if empty {
                    occupied.remove();
                }
            }
        }

        // remove dep
        for d in entry.related_dep_out_points().cloned() {
            if let Entry::Occupied(mut occupied) = self.deps.entry(d) {
                let empty = {
                    let ids = occupied.get_mut();
                    ids.remove(&tx_short_id);
                    ids.is_empty()
                };
                if empty {
                    occupied.remove();
                }
            }
        }

        for o in outputs {
            self.outputs.remove(&o);
        }

        self.header_deps.remove(&tx_short_id);
    }

    pub(crate) fn remove_entries_by_filter<P: FnMut(&ProposalShortId, &TxEntry) -> bool>(
        &mut self,
        mut predicate: P,
    ) -> Vec<TxEntry> {
        let entries = self.entries();
        let mut removed = Vec::new();
        for entry in entries {
            if predicate(entry.key(), entry.get()) {
                removed.push(entry.remove());
            }
        }
        for entry in &removed {
            self.remove_entry_relation(entry);
        }

        removed
    }

    pub fn entries(&mut self) -> LinkedHashMapEntries<ProposalShortId, TxEntry> {
        self.inner.entries()
    }

    // fill proposal txs
    pub fn fill_proposals(
        &self,
        limit: usize,
        exclusion: &HashSet<ProposalShortId>,
        proposals: &mut HashSet<ProposalShortId>,
    ) {
        for id in self.inner.keys() {
            if proposals.len() == limit {
                break;
            }
            if !exclusion.contains(id) {
                proposals.insert(id.clone());
            }
        }
    }

    pub(crate) fn drain(&mut self) -> Vec<TransactionView> {
        let txs = self
            .inner
            .drain()
            .map(|(_k, entry)| entry.into_transaction())
            .collect::<Vec<_>>();
        self.deps.clear();
        self.inputs.clear();
        self.header_deps.clear();
        self.outputs.clear();
        txs
    }
}

impl CellProvider for PendingQueue {
    fn cell(&self, out_point: &OutPoint, _eager_load: bool) -> CellStatus {
        let tx_hash = out_point.tx_hash();
        if let Some(entry) = self.inner.get(&ProposalShortId::from_tx_hash(&tx_hash)) {
            match entry
                .transaction()
                .output_with_data(out_point.index().unpack())
            {
                Some((output, data)) => {
                    let cell_meta = CellMetaBuilder::from_cell_output(output, data)
                        .out_point(out_point.to_owned())
                        .build();
                    CellStatus::live_cell(cell_meta)
                }
                None => CellStatus::Unknown,
            }
        } else {
            CellStatus::Unknown
        }
    }
}

impl CellChecker for PendingQueue {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        let tx_hash = out_point.tx_hash();
        if let Some(entry) = self.inner.get(&ProposalShortId::from_tx_hash(&tx_hash)) {
            entry
                .transaction()
                .output(out_point.index().unpack())
                .map(|_| true)
        } else {
            None
        }
    }
}
