use crate::component::container::SortedTxMap;
use crate::component::entry::TxEntry;
use crate::error::Reject;
use ckb_types::{
    bytes::Bytes,
    core::{
        cell::{CellChecker, CellMetaBuilder, CellProvider, CellStatus},
        error::OutPointError,
        TransactionView,
    },
    packed::{Byte32, CellOutput, OutPoint, ProposalShortId},
    prelude::*,
};
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::iter;

type ConflictEntry = (TxEntry, Reject);

#[derive(Default, Debug, Clone)]
pub(crate) struct Edges {
    /// output-op<txid> map represent in-pool tx's outputs
    pub(crate) outputs: HashMap<OutPoint, Option<ProposalShortId>>,
    /// input-txid map represent in-pool tx's inputs
    pub(crate) inputs: HashMap<OutPoint, ProposalShortId>,
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

    pub(crate) fn insert_input(&mut self, out_point: OutPoint, txid: ProposalShortId) {
        self.inputs.insert(out_point, txid);
    }

    pub(crate) fn remove_input(&mut self, out_point: &OutPoint) -> Option<ProposalShortId> {
        self.inputs.remove(out_point)
    }

    pub(crate) fn remove_output(&mut self, out_point: &OutPoint) -> Option<ProposalShortId> {
        self.outputs.remove(out_point).unwrap_or(None)
    }

    pub(crate) fn insert_output(&mut self, out_point: OutPoint) {
        self.outputs.insert(out_point, None);
    }

    pub(crate) fn insert_consumed_output(&mut self, out_point: OutPoint, id: ProposalShortId) {
        self.outputs.insert(out_point, Some(id));
    }

    pub(crate) fn get_output_ref(&self, out_point: &OutPoint) -> Option<&Option<ProposalShortId>> {
        self.outputs.get(out_point)
    }

    pub(crate) fn get_input_ref(&self, out_point: &OutPoint) -> Option<&ProposalShortId> {
        self.inputs.get(out_point)
    }

    pub(crate) fn get_deps_ref(&self, out_point: &OutPoint) -> Option<&HashSet<ProposalShortId>> {
        self.deps.get(out_point)
    }

    pub(crate) fn get_mut_output(
        &mut self,
        out_point: &OutPoint,
    ) -> Option<&mut Option<ProposalShortId>> {
        self.outputs.get_mut(out_point)
    }

    pub(crate) fn remove_deps(&mut self, out_point: &OutPoint) -> Option<HashSet<ProposalShortId>> {
        self.deps.remove(out_point)
    }

    pub(crate) fn insert_deps(&mut self, out_point: OutPoint, txid: ProposalShortId) {
        self.deps.entry(out_point).or_default().insert(txid);
    }

    pub(crate) fn delete_txid_by_dep(&mut self, out_point: OutPoint, txid: &ProposalShortId) {
        if let Entry::Occupied(mut occupied) = self.deps.entry(out_point) {
            let empty = {
                let ids = occupied.get_mut();
                ids.remove(txid);
                ids.is_empty()
            };
            if empty {
                occupied.remove();
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.outputs.clear();
        self.inputs.clear();
        self.deps.clear();
        self.header_deps.clear();
    }
}

#[derive(Clone)]
pub struct ProposedPool {
    pub(crate) edges: Edges,
    inner: SortedTxMap,
}

impl CellProvider for ProposedPool {
    fn cell(&self, out_point: &OutPoint, _eager_load: bool) -> CellStatus {
        if self.edges.get_input_ref(out_point).is_some() {
            return CellStatus::Dead;
        }
        if let Some(x) = self.edges.get_output_ref(out_point) {
            // output consumed
            if x.is_some() {
                return CellStatus::Dead;
            } else {
                let (output, data) = self.get_output_with_data(out_point).expect("output");
                let cell_meta = CellMetaBuilder::from_cell_output(output, data)
                    .out_point(out_point.to_owned())
                    .build();
                return CellStatus::live_cell(cell_meta);
            }
        }
        CellStatus::Unknown
    }
}

impl CellChecker for ProposedPool {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        if self.edges.get_input_ref(out_point).is_some() {
            return Some(false);
        }
        if let Some(x) = self.edges.get_output_ref(out_point) {
            // output consumed
            if x.is_some() {
                return Some(false);
            } else {
                return Some(true);
            }
        }
        None
    }
}

impl ProposedPool {
    pub(crate) fn new(max_ancestors_count: usize) -> Self {
        ProposedPool {
            edges: Default::default(),
            inner: SortedTxMap::new(max_ancestors_count),
        }
    }

    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.contains_key(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.inner.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ProposalShortId, &TxEntry)> {
        self.inner.iter()
    }

    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.get(id).map(|entry| entry.transaction())
    }

    pub fn size(&self) -> usize {
        self.inner.size()
    }

    pub(crate) fn get_output_with_data(&self, out_point: &OutPoint) -> Option<(CellOutput, Bytes)> {
        self.inner
            .get(&ProposalShortId::from_tx_hash(&out_point.tx_hash()))
            .and_then(|entry| {
                entry
                    .transaction()
                    .output_with_data(out_point.index().unpack())
            })
    }

    // remove entry and all it's descendants
    pub(crate) fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        let removed_entries = self.inner.remove_entry_and_descendants(id);
        for entry in &removed_entries {
            let tx = entry.transaction();
            let inputs = tx.input_pts_iter();
            let outputs = tx.output_pts();
            for i in inputs {
                self.edges.inputs.remove(&i);
                if let Some(id) = self.edges.get_mut_output(&i) {
                    *id = None;
                }
            }

            for d in entry.related_dep_out_points().cloned() {
                self.edges.delete_txid_by_dep(d, id);
            }

            for o in outputs {
                self.edges.remove_output(&o);
            }

            self.edges.header_deps.remove(&entry.proposal_short_id());
        }
        removed_entries
    }

    pub(crate) fn remove_committed_tx(&mut self, tx: &TransactionView) -> Option<TxEntry> {
        let outputs = tx.output_pts();
        let inputs = tx.input_pts_iter();
        let id = tx.proposal_short_id();

        if let Some(entry) = self.inner.remove_entry(&id) {
            for o in outputs {
                self.edges.remove_output(&o);
            }

            for i in inputs {
                // release input record
                self.edges.remove_input(&i);
                if let Some(id) = self.edges.get_mut_output(&i) {
                    *id = None;
                }
            }

            for d in entry.related_dep_out_points().cloned() {
                self.edges.delete_txid_by_dep(d, &id);
            }

            self.edges.header_deps.remove(&id);

            return Some(entry);
        }
        None
    }

    // In the event of a reorg, the assumption that a newly added tx has no
    // in-pool children is false. In particular, the pool is in an
    // inconsistent state while new transactions are being added, because there may
    // be descendant transactions of a tx coming from a disconnected block that are
    // unreachable from just looking at transactions in the pool (the linking
    // transactions may also be in the disconnected block, waiting to be added).
    // Because of this, there's not much benefit in trying to search for in-pool
    // children in add_entry().  Instead, in the special case of transactions
    // being added from a disconnected block, out-of-block descendants for all the
    // in-block transactions by calling update_descendants_from_detached().  Note that
    // until this is called, the pool state is not consistent, and in particular
    // TxLinks may not be correct (and therefore functions like
    // calc_ancestors() and calc_descendants() that rely
    // on them to walk the pool are not generally safe to use).
    pub(crate) fn add_entry(&mut self, entry: TxEntry) -> Result<bool, Reject> {
        let tx_short_id = entry.proposal_short_id();

        if self.inner.contains_key(&tx_short_id) {
            return Ok(false);
        }

        let inputs = entry.transaction().input_pts_iter();
        let outputs = entry.transaction().output_pts();
        let related_dep_out_points: Vec<_> = entry.related_dep_out_points().cloned().collect();
        let header_deps = entry.transaction().header_deps();

        self.inner.add_entry(entry).map(|inserted| {
            if inserted {
                let mut children = HashSet::new();
                // if input reference a in-pool output, connect it
                // otherwise, record input for conflict check
                for i in inputs {
                    if let Some(id) = self.edges.get_mut_output(&i) {
                        *id = Some(tx_short_id.clone());
                    }
                    self.edges.insert_input(i.to_owned(), tx_short_id.clone());
                }

                // record dep-txid
                for d in related_dep_out_points {
                    self.edges.insert_deps(d.to_owned(), tx_short_id.clone());
                }

                // record tx output
                for o in outputs {
                    if let Some(ids) = self.edges.get_deps_ref(&o).cloned() {
                        children.extend(ids);
                    }
                    if let Some(id) = self.edges.get_input_ref(&o).cloned() {
                        self.edges.insert_consumed_output(o, id.clone());
                        children.insert(id);
                    } else {
                        self.edges.insert_output(o);
                    }
                }

                // record header_deps
                if !header_deps.is_empty() {
                    self.edges
                        .header_deps
                        .insert(tx_short_id.clone(), header_deps.into_iter().collect());
                }

                if !children.is_empty() {
                    self.inner
                        .update_descendants_from_detached(&tx_short_id, children);
                }
            }
            inserted
        })
    }

    pub(crate) fn resolve_conflict(&mut self, tx: &TransactionView) -> Vec<ConflictEntry> {
        let inputs = tx.input_pts_iter();
        let mut conflicts = Vec::new();

        for i in inputs {
            if let Some(id) = self.edges.remove_input(&i) {
                let entries = self.remove_entry_and_descendants(&id);
                if !entries.is_empty() {
                    let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                    let rejects = iter::repeat(reject).take(entries.len());
                    conflicts.extend(entries.into_iter().zip(rejects));
                }
            }

            // deps consumed
            if let Some(x) = self.edges.remove_deps(&i) {
                for id in x {
                    let entries = self.remove_entry_and_descendants(&id);
                    if !entries.is_empty() {
                        let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                        let rejects = iter::repeat(reject).take(entries.len());
                        conflicts.extend(entries.into_iter().zip(rejects));
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
        let mut invalid_header_ids = Vec::new();
        for (tx_id, deps) in self.edges.header_deps.iter() {
            for hash in deps {
                if headers.contains(hash) {
                    invalid_header_ids.push((hash.clone(), tx_id.clone()));
                    break;
                }
            }
        }

        for (blk_hash, id) in invalid_header_ids {
            let entries = self.remove_entry_and_descendants(&id);
            if !entries.is_empty() {
                let reject = Reject::Resolve(OutPointError::InvalidHeader(blk_hash));
                let rejects = iter::repeat(reject).take(entries.len());
                conflicts.extend(entries.into_iter().zip(rejects));
            }
        }

        conflicts
    }

    /// sorted by ancestor score from higher to lower
    pub fn score_sorted_iter(&self) -> impl Iterator<Item = &TxEntry> {
        self.inner.score_sorted_iter()
    }

    /// find all ancestors from pool
    pub fn calc_ancestors(&self, tx_short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.inner.calc_ancestors(tx_short_id)
    }

    /// find all descendants from pool
    pub fn calc_descendants(&self, tx_short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.inner.calc_descendants(tx_short_id)
    }

    #[cfg(test)]
    pub(crate) fn inner(&self) -> &SortedTxMap {
        &self.inner
    }

    pub(crate) fn clear(&mut self) {
        self.edges.clear();
        self.inner.clear();
    }
}
