use crate::component::entry::TxEntry;
use ckb_types::{
    core::{
        cell::{CellChecker, CellMetaBuilder, CellProvider, CellStatus},
        error::OutPointError,
        tx_pool::Reject,
        TransactionView,
    },
    packed::{OutPoint, ProposalShortId},
    prelude::*,
};
use ckb_util::{LinkedHashMap, LinkedHashMapEntries};
use std::collections::{HashMap, HashSet};

type ConflictEntry = (TxEntry, Reject);

#[derive(Debug, Clone)]
pub(crate) struct PendingQueue {
    pub(crate) inner: LinkedHashMap<ProposalShortId, TxEntry>,
    /// dep-set<txid> map represent in-pool tx's deps
    pub(crate) deps: HashMap<OutPoint, HashSet<ProposalShortId>>,
    /// input-txid map represent in-pool tx's inputs
    pub(crate) inputs: HashMap<OutPoint, ProposalShortId>,
}

impl PendingQueue {
    pub(crate) fn new() -> Self {
        PendingQueue {
            inner: Default::default(),
            deps: Default::default(),
            inputs: Default::default(),
        }
    }

    pub(crate) fn size(&self) -> usize {
        self.inner.len()
    }

    pub(crate) fn add_entry(&mut self, entry: TxEntry) -> bool {
        let inputs = entry.transaction().input_pts_iter();
        let tx_short_id = entry.proposal_short_id();

        if self.inner.contains_key(&tx_short_id) {
            return false;
        }

        for i in inputs {
            self.inputs.insert(i.to_owned(), tx_short_id.clone());
        }

        // record dep-txid
        for d in entry.related_dep_out_points() {
            self.deps
                .entry(d.to_owned())
                .or_default()
                .insert(tx_short_id.clone());
        }

        self.inner.insert(tx_short_id, entry);
        true
    }

    pub(crate) fn resolve_conflict(
        &mut self,
        tx: &TransactionView,
    ) -> (Vec<ConflictEntry>, Vec<ConflictEntry>) {
        let inputs = tx.input_pts_iter();
        let mut input_conflict = Vec::new();
        let mut deps_consumed = Vec::new();

        for i in inputs {
            if let Some(id) = self.inputs.remove(&i) {
                if let Some(entry) = self.remove_entry(&id) {
                    let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                    input_conflict.push((entry, reject));
                }
            }

            // deps consumed
            if let Some(x) = self.deps.remove(&i) {
                for id in x {
                    if let Some(entry) = self.remove_entry(&id) {
                        let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                        deps_consumed.push((entry, reject));
                    }
                }
            }
        }
        (input_conflict, deps_consumed)
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

    pub(crate) fn remove_committed_tx(
        &mut self,
        tx: &TransactionView,
        related_out_points: &[OutPoint],
    ) -> Option<TxEntry> {
        let inputs = tx.input_pts_iter();
        let id = tx.proposal_short_id();

        if let Some(entry) = self.inner.remove(&id) {
            for i in inputs {
                self.inputs.remove(&i);
            }

            for d in related_out_points {
                let mut empty = false;
                if let Some(x) = self.deps.get_mut(d) {
                    x.remove(&id);
                    empty = x.is_empty();
                }

                if empty {
                    self.deps.remove(d);
                }
            }

            return Some(entry);
        }
        None
    }

    pub(crate) fn remove_entry(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        let removed = self.inner.remove(id);

        if let Some(ref entry) = removed {
            self.remove_entry_relation(entry);
        }

        removed
    }

    pub(crate) fn remove_entry_relation(&mut self, entry: &TxEntry) {
        let inputs = entry.transaction().input_pts_iter();
        let tx_short_id = entry.proposal_short_id();

        for i in inputs {
            self.inputs.remove(&i);
        }

        // remove dep
        for d in entry.related_dep_out_points() {
            let mut empty = false;
            if let Some(x) = self.deps.get_mut(d) {
                x.remove(&tx_short_id);
                empty = x.is_empty();
            }

            if empty {
                self.deps.remove(d);
            }
        }
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
            self.remove_entry_relation(&entry);
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
            if !exclusion.contains(&id) {
                proposals.insert(id.clone());
            }
        }
    }

    pub(crate) fn drain(&mut self) -> Vec<TransactionView> {
        let txs = self
            .inner
            .values()
            .map(|entry| entry.transaction().clone())
            .collect::<Vec<_>>();
        self.inner.clear();
        self.deps.clear();
        self.inputs.clear();
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
