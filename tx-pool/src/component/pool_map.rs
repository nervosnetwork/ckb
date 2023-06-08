//! Top-level Pool type, methods, and tests
extern crate rustc_hash;
extern crate slab;
use crate::component::edges::{Edges, OutPointStatus};
use crate::component::entry::EvictKey;
use crate::component::links::{Relation, TxLinksMap};
use crate::component::score_key::AncestorsScoreSortKey;
use crate::error::Reject;
use crate::TxEntry;

use ckb_logger::trace;
use ckb_multi_index_map::MultiIndexMap;
use ckb_types::core::error::OutPointError;
use ckb_types::packed::OutPoint;
use ckb_types::{
    bytes::Bytes,
    core::{cell::CellChecker, TransactionView},
    packed::{Byte32, CellOutput, ProposalShortId},
};
use ckb_types::{
    core::cell::{CellMetaBuilder, CellProvider, CellStatus},
    prelude::*,
};
use std::borrow::Cow;
use std::collections::HashSet;

use super::links::TxLinks;

type ConflictEntry = (TxEntry, Reject);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    Pending,
    Gap,
    Proposed,
}

#[derive(Copy, Clone)]
enum EntryOp {
    Add,
    Remove,
}

#[derive(MultiIndexMap, Clone)]
pub struct PoolEntry {
    #[multi_index(hashed_unique)]
    pub id: ProposalShortId,
    #[multi_index(ordered_non_unique)]
    pub score: AncestorsScoreSortKey,
    #[multi_index(ordered_non_unique)]
    pub status: Status,
    #[multi_index(ordered_non_unique)]
    pub evict_key: EvictKey,
    // other sort key
    pub inner: TxEntry,
}

pub struct PoolMap {
    /// The pool entries with different kinds of sort strategies
    pub(crate) entries: MultiIndexPoolEntryMap,
    /// All the deps, header_deps, inputs, outputs relationships
    pub(crate) edges: Edges,
    /// All the parent/children relationships
    pub(crate) links: TxLinksMap,
    pub(crate) max_ancestors_count: usize,
}

impl PoolMap {
    pub fn new(max_ancestors_count: usize) -> Self {
        PoolMap {
            entries: MultiIndexPoolEntryMap::default(),
            edges: Edges::default(),
            links: TxLinksMap::new(),
            max_ancestors_count,
        }
    }

    #[cfg(test)]
    pub(crate) fn outputs_len(&self) -> usize {
        self.edges.outputs_len()
    }

    #[cfg(test)]
    pub(crate) fn header_deps_len(&self) -> usize {
        self.edges.header_deps_len()
    }

    #[cfg(test)]
    pub(crate) fn deps_len(&self) -> usize {
        self.edges.deps_len()
    }

    #[cfg(test)]
    pub(crate) fn inputs_len(&self) -> usize {
        self.edges.inputs_len()
    }

    #[cfg(test)]
    pub(crate) fn size(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.entries.get_by_id(id).is_some()
    }

    #[cfg(test)]
    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.entries
            .get_by_id(id)
            .map(|entry| entry.inner.transaction())
    }

    #[cfg(test)]
    pub(crate) fn add_proposed(&mut self, entry: TxEntry) -> Result<bool, Reject> {
        self.add_entry(entry, Status::Proposed)
    }

    #[cfg(test)]
    pub(crate) fn remove_committed_tx(&mut self, tx: &TransactionView) -> Option<TxEntry> {
        self.remove_entry(&tx.proposal_short_id())
    }

    pub(crate) fn get_by_id(&self, id: &ProposalShortId) -> Option<&PoolEntry> {
        self.entries.get_by_id(id)
    }

    pub(crate) fn pending_size(&self) -> usize {
        self.entries.get_by_status(&Status::Pending).len()
            + self.entries.get_by_status(&Status::Gap).len()
    }

    pub(crate) fn proposed_size(&self) -> usize {
        self.entries.get_by_status(&Status::Proposed).len()
    }

    pub(crate) fn score_sorted_iter(&self) -> impl Iterator<Item = &TxEntry> {
        self.entries
            .iter_by_score()
            .rev()
            .filter(|entry| entry.status == Status::Proposed)
            .map(|entry| &entry.inner)
    }

    pub(crate) fn get(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.get_by_id(id).map(|entry| &entry.inner)
    }

    pub(crate) fn get_proposed(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        match self.get_by_id(id) {
            Some(entry) if entry.status == Status::Proposed => Some(&entry.inner),
            _ => None,
        }
    }

    pub(crate) fn has_proposed(&self, id: &ProposalShortId) -> bool {
        self.get_proposed(id).is_some()
    }

    /// calculate all ancestors from pool
    pub(crate) fn calc_ancestors(&self, short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.links.calc_ancestors(short_id)
    }

    /// calculate all descendants from pool
    pub(crate) fn calc_descendants(&self, short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.links.calc_descendants(short_id)
    }

    pub(crate) fn get_output_with_data(&self, out_point: &OutPoint) -> Option<(CellOutput, Bytes)> {
        self.get(&ProposalShortId::from_tx_hash(&out_point.tx_hash()))
            .and_then(|entry| {
                entry
                    .transaction()
                    .output_with_data(out_point.index().unpack())
            })
    }

    pub(crate) fn add_entry(&mut self, mut entry: TxEntry, status: Status) -> Result<bool, Reject> {
        let tx_short_id = entry.proposal_short_id();
        if self.entries.get_by_id(&tx_short_id).is_some() {
            return Ok(false);
        }
        trace!("pool_map.add_{:?} {}", status, entry.transaction().hash());
        self.record_entry_links(&mut entry)?;
        self.insert_entry(&entry, status)?;
        self.record_entry_deps(&entry);
        self.record_entry_edges(&entry);
        Ok(true)
    }

    /// Change the status of the entry, only used for `gap_rtx` and `proposed_rtx`
    pub(crate) fn set_entry(&mut self, entry: &TxEntry, status: Status) {
        let tx_short_id = entry.proposal_short_id();
        let _ = self
            .entries
            .get_by_id(&tx_short_id)
            .expect("unconsistent pool");
        self.entries.modify_by_id(&tx_short_id, |e| {
            e.status = status;
        });
    }

    pub(crate) fn remove_entry(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        if let Some(entry) = self.entries.remove_by_id(id) {
            self.update_descendants_index_key(&entry.inner, EntryOp::Remove);
            self.remove_entry_deps(&entry.inner);
            self.remove_entry_edges(&entry.inner);
            self.remove_entry_links(id);
            return Some(entry.inner);
        }
        None
    }

    pub(crate) fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        let mut removed_ids = vec![id.to_owned()];
        let mut removed = vec![];
        removed_ids.extend(self.calc_descendants(id));

        // update links state for remove, so that we won't update_descendants_index_key in remove_entry
        for id in &removed_ids {
            self.remove_entry_links(id);
        }

        for id in removed_ids {
            if let Some(entry) = self.remove_entry(&id) {
                removed.push(entry);
            }
        }
        removed
    }

    pub(crate) fn resolve_conflict_header_dep(
        &mut self,
        headers: &HashSet<Byte32>,
    ) -> Vec<ConflictEntry> {
        let mut conflicts = Vec::new();

        // invalid header deps
        let mut ids = Vec::new();
        for (tx_id, deps) in self.edges.header_deps.iter() {
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

    pub(crate) fn resolve_conflict(&mut self, tx: &TransactionView) -> Vec<ConflictEntry> {
        let inputs = tx.input_pts_iter();
        let mut conflicts = Vec::new();

        for i in inputs {
            if let Some(id) = self.edges.remove_input(&i) {
                let entries = self.remove_entry_and_descendants(&id);
                if !entries.is_empty() {
                    let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                    let rejects = std::iter::repeat(reject).take(entries.len());
                    conflicts.extend(entries.into_iter().zip(rejects));
                }
            }

            // deps consumed
            if let Some(x) = self.edges.remove_deps(&i) {
                for id in x {
                    let entries = self.remove_entry_and_descendants(&id);
                    if !entries.is_empty() {
                        let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                        let rejects = std::iter::repeat(reject).take(entries.len());
                        conflicts.extend(entries.into_iter().zip(rejects));
                    }
                }
            }
        }

        conflicts
    }

    // fill proposal txs
    pub(crate) fn fill_proposals(
        &self,
        limit: usize,
        exclusion: &HashSet<ProposalShortId>,
        proposals: &mut HashSet<ProposalShortId>,
        status: &Status,
    ) {
        for entry in self.entries.get_by_status(status) {
            if proposals.len() == limit {
                break;
            }
            if !exclusion.contains(&entry.id) {
                proposals.insert(entry.id.clone());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn remove_entries_by_filter<P: FnMut(&ProposalShortId, &TxEntry) -> bool>(
        &mut self,
        status: &Status,
        mut predicate: P,
    ) -> Vec<TxEntry> {
        let mut removed = Vec::new();
        for entry in self.entries.get_by_status(status) {
            if predicate(&entry.id, &entry.inner) {
                removed.push(entry.inner.clone());
            }
        }
        for entry in &removed {
            self.remove_entry(&entry.proposal_short_id());
        }
        removed
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &PoolEntry> {
        self.entries.iter().map(|(_, entry)| entry)
    }

    pub(crate) fn next_evict_entry(&self) -> Option<ProposalShortId> {
        self.entries.iter_by_evict_key()
            .next()
            .map(|entry| entry.id.clone())
    }

    pub(crate) fn clear(&mut self) {
        self.entries = MultiIndexPoolEntryMap::default();
        self.edges.clear();
        self.links.clear();
    }

    fn remove_entry_links(&mut self, id: &ProposalShortId) {
        if let Some(parents) = self.links.get_parents(id).cloned() {
            for parent in parents {
                self.links.remove_child(&parent, id);
            }
        }
        if let Some(children) = self.links.get_children(id).cloned() {
            for child in children {
                self.links.remove_parent(&child, id);
            }
        }
        self.links.remove(id);
    }

    fn update_descendants_index_key(&mut self, parent: &TxEntry, op: EntryOp) {
        let descendants: HashSet<ProposalShortId> =
            self.links.calc_descendants(&parent.proposal_short_id());
        for desc_id in &descendants {
            // update child score
            let entry = self.entries.get_by_id(desc_id).unwrap().clone();
            let mut child = entry.inner.clone();
            match op {
                EntryOp::Remove => child.sub_entry_weight(parent),
                EntryOp::Add => child.add_entry_weight(parent),
            }
            let short_id = child.proposal_short_id();
            self.entries.modify_by_id(&short_id, |e| {
                e.score = child.as_score_key();
                e.evict_key = child.as_evict_key();
                e.inner = child;
            });
        }
    }

    fn record_entry_deps(&mut self, entry: &TxEntry) {
        let tx_short_id: ProposalShortId = entry.proposal_short_id();
        let header_deps = entry.transaction().header_deps();
        let related_dep_out_points: Vec<_> = entry.related_dep_out_points().cloned().collect();

        // record dep-txid
        for d in related_dep_out_points {
            self.edges.insert_deps(d.to_owned(), tx_short_id.clone());
        }
        // record header_deps
        if !header_deps.is_empty() {
            self.edges
                .header_deps
                .insert(tx_short_id, header_deps.into_iter().collect());
        }
    }

    fn record_entry_edges(&mut self, entry: &TxEntry) {
        let tx_short_id: ProposalShortId = entry.proposal_short_id();
        let inputs = entry.transaction().input_pts_iter();
        let outputs = entry.transaction().output_pts();

        let mut children = HashSet::new();
        // if input reference a in-pool output, connect it
        // otherwise, record input for conflict check
        for i in inputs {
            self.edges.set_output_consumed(&i, &tx_short_id);
            self.edges.insert_input(i.to_owned(), tx_short_id.clone());
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
                self.edges.insert_unconsumed_output(o);
            }
        }
        // update children
        if !children.is_empty() {
            self.update_descendants_from_detached(&tx_short_id, children);
        }
    }

    // update_descendants_from_detached is used to update
    // the descendants for a single transaction that has been added to the
    // pool but may have child transactions in the pool, eg during a
    // chain reorg.
    fn update_descendants_from_detached(
        &mut self,
        id: &ProposalShortId,
        children: HashSet<ProposalShortId>,
    ) {
        if let Some(entry) = self.get_by_id(id).cloned() {
            for child in &children {
                self.links.add_parent(child, id.clone());
            }
            if let Some(links) = self.links.inner.get_mut(id) {
                links.children.extend(children);
            }

            self.update_descendants_index_key(&entry.inner, EntryOp::Add);
        }
    }

    /// Record the links for entry
    fn record_entry_links(&mut self, entry: &mut TxEntry) -> Result<bool, Reject> {
        // find in pool parents
        let mut parents: HashSet<ProposalShortId> = HashSet::with_capacity(
            entry.transaction().inputs().len() + entry.transaction().cell_deps().len(),
        );
        let short_id = entry.proposal_short_id();

        for input in entry.transaction().inputs() {
            let input_pt = input.previous_output();
            if let Some(deps) = self.edges.deps.get(&input_pt) {
                parents.extend(deps.iter().cloned());
            }

            let parent_hash = &input_pt.tx_hash();
            let id = ProposalShortId::from_tx_hash(parent_hash);
            if self.links.inner.contains_key(&id) {
                parents.insert(id);
            }
        }
        for cell_dep in entry.transaction().cell_deps() {
            let dep_pt = cell_dep.out_point();
            let id = ProposalShortId::from_tx_hash(&dep_pt.tx_hash());
            if self.links.inner.contains_key(&id) {
                parents.insert(id);
            }
        }

        let ancestors = self
            .links
            .calc_relation_ids(Cow::Borrowed(&parents), Relation::Parents);

        // update parents references
        for ancestor_id in &ancestors {
            let ancestor = self
                .entries
                .get_by_id(ancestor_id)
                .expect("pool consistent");
            entry.add_entry_weight(&ancestor.inner);
        }
        if entry.ancestors_count > self.max_ancestors_count {
            eprintln!("debug: exceeded maximum ancestors count");
            return Err(Reject::ExceededMaximumAncestorsCount);
        }

        for cell_dep in entry.transaction().cell_deps() {
            let dep_pt = cell_dep.out_point();
            // insert dep-ref map
            self.edges
                .deps
                .entry(dep_pt)
                .or_insert_with(HashSet::new)
                .insert(short_id.clone());
        }

        for parent in &parents {
            self.links.add_child(parent, short_id.clone());
        }

        // insert links
        let links = TxLinks {
            parents,
            children: Default::default(),
        };
        self.links.inner.insert(short_id, links);

        Ok(true)
    }

    fn remove_entry_edges(&mut self, entry: &TxEntry) {
        let inputs = entry.transaction().input_pts_iter();
        let outputs = entry.transaction().output_pts();

        for o in outputs {
            self.edges.remove_output(&o);
        }

        for i in inputs {
            // release input record
            self.edges.remove_input(&i);
            self.edges.set_output_unconsumed(&i);
        }
    }

    fn remove_entry_deps(&mut self, entry: &TxEntry) {
        let id = entry.proposal_short_id();
        for d in entry.related_dep_out_points().cloned() {
            self.edges.delete_txid_by_dep(d, &id);
        }

        self.edges.header_deps.remove(&id);
    }

    fn insert_entry(&mut self, entry: &TxEntry, status: Status) -> Result<bool, Reject> {
        let tx_short_id = entry.proposal_short_id();
        let score = entry.as_score_key();
        let evict_key = entry.as_evict_key();
        self.entries.insert(PoolEntry {
            id: tx_short_id,
            score,
            status,
            inner: entry.clone(),
            evict_key,
        });
        Ok(true)
    }
}

impl CellProvider for PoolMap {
    fn cell(&self, out_point: &OutPoint, _eager_load: bool) -> CellStatus {
        if self.edges.get_input_ref(out_point).is_some() {
            return CellStatus::Dead;
        }
        match self.edges.get_output_ref(out_point) {
            Some(OutPointStatus::UnConsumed) => {
                let (output, data) = self.get_output_with_data(out_point).expect("output");
                let cell_meta = CellMetaBuilder::from_cell_output(output, data)
                    .out_point(out_point.to_owned())
                    .build();
                CellStatus::live_cell(cell_meta)
            }
            Some(OutPointStatus::Consumed(_id)) => CellStatus::Dead,
            _ => CellStatus::Unknown,
        }
    }
}

impl CellChecker for PoolMap {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        if self.edges.get_input_ref(out_point).is_some() {
            return Some(false);
        }
        match self.edges.get_output_ref(out_point) {
            Some(OutPointStatus::Consumed(_id)) => Some(false),
            Some(OutPointStatus::UnConsumed) => Some(true),
            _ => None,
        }
    }
}
