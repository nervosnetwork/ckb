//! Top-level Pool type, methods, and tests
extern crate rustc_hash;
extern crate slab;
use crate::component::edges::Edges;
use crate::component::links::{Relation, TxLinksMap};
use crate::component::sort_key::{AncestorsScoreSortKey, EvictKey};
use crate::error::Reject;
use crate::TxEntry;

use ckb_logger::trace;
use ckb_types::core::error::OutPointError;
use ckb_types::packed::OutPoint;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::TransactionView,
    packed::{Byte32, CellOutput, ProposalShortId},
};
use multi_index_map::MultiIndexMap;
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

    pub(crate) fn get_by_id(&self, id: &ProposalShortId) -> Option<&PoolEntry> {
        self.entries.get_by_id(id)
    }

    fn get_by_id_checked(&self, id: &ProposalShortId) -> &PoolEntry {
        self.get_by_id(id).expect("unconsistent pool")
    }

    pub(crate) fn get_by_status(&self, status: Status) -> Vec<&PoolEntry> {
        self.entries.get_by_status(&status)
    }

    pub(crate) fn pending_size(&self) -> usize {
        self.entries.get_by_status(&Status::Pending).len()
            + self.entries.get_by_status(&Status::Gap).len()
    }

    pub(crate) fn proposed_size(&self) -> usize {
        self.entries.get_by_status(&Status::Proposed).len()
    }

    pub(crate) fn sorted_proposed_iter(&self) -> impl Iterator<Item = &TxEntry> {
        self.score_sorted_iter_by(vec![Status::Proposed])
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
        self.check_and_record_ancestors(&mut entry)?;
        self.insert_entry(&entry, status);
        self.record_entry_edges(&entry);
        self.record_entry_descendants(&entry);
        Ok(true)
    }

    /// Change the status of the entry, only used for `gap_rtx` and `proposed_rtx`
    pub(crate) fn set_entry(&mut self, short_id: &ProposalShortId, status: Status) {
        self.entries
            .modify_by_id(short_id, |e| {
                e.status = status;
            })
            .expect("unconsistent pool");
    }

    pub(crate) fn remove_entry(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        self.entries.remove_by_id(id).map(|entry| {
            self.update_ancestors_index_key(&entry.inner, EntryOp::Remove);
            self.update_descendants_index_key(&entry.inner, EntryOp::Remove);
            self.remove_entry_edges(&entry.inner);
            self.remove_entry_links(id);
            entry.inner
        })
    }

    pub(crate) fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        let mut removed_ids = vec![id.to_owned()];
        removed_ids.extend(self.calc_descendants(id));

        // update links state for remove, so that we won't update_descendants_index_key in remove_entry
        for id in &removed_ids {
            self.remove_entry_links(id);
        }

        removed_ids
            .iter()
            .filter_map(|id| self.remove_entry(id))
            .collect()
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

    pub(crate) fn find_conflict_tx(&self, tx: &TransactionView) -> HashSet<ProposalShortId> {
        let mut res = HashSet::default();
        for i in tx.input_pts_iter() {
            if let Some(id) = self.edges.get_input_ref(&i) {
                res.insert(id.clone());
            }
        }
        res
    }

    pub(crate) fn resolve_conflict(&mut self, tx: &TransactionView) -> Vec<ConflictEntry> {
        let mut conflicts = Vec::new();

        for i in tx.input_pts_iter() {
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
        status: Status,
    ) {
        for entry in self.score_sorted_iter_by(vec![status]) {
            if proposals.len() == limit {
                break;
            }
            let id = entry.proposal_short_id();
            if !exclusion.contains(&id) {
                proposals.insert(id);
            }
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &PoolEntry> {
        self.entries.iter().map(|(_, entry)| entry)
    }

    pub(crate) fn next_evict_entry(&self, status: Status) -> Option<ProposalShortId> {
        self.entries
            .iter_by_evict_key()
            .find(move |entry| entry.status == status)
            .map(|entry| entry.id.clone())
    }

    pub(crate) fn clear(&mut self) {
        self.entries = MultiIndexPoolEntryMap::default();
        self.edges.clear();
        self.links.clear();
    }

    pub(crate) fn score_sorted_iter_by(
        &self,
        statuses: Vec<Status>,
    ) -> impl Iterator<Item = &TxEntry> {
        self.entries
            .iter_by_score()
            .rev()
            .filter(move |entry| statuses.contains(&entry.status))
            .map(|entry| &entry.inner)
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

    fn update_ancestors_index_key(&mut self, child: &TxEntry, op: EntryOp) {
        let ancestors: HashSet<ProposalShortId> =
            self.links.calc_ancestors(&child.proposal_short_id());
        for anc_id in &ancestors {
            // update parent score
            self.entries.modify_by_id(anc_id, |e| {
                match op {
                    EntryOp::Remove => e.inner.sub_descendant_weight(child),
                    EntryOp::Add => e.inner.add_descendant_weight(child),
                };
                e.evict_key = e.inner.as_evict_key();
            });
        }
    }

    fn update_descendants_index_key(&mut self, parent: &TxEntry, op: EntryOp) {
        let descendants: HashSet<ProposalShortId> =
            self.links.calc_descendants(&parent.proposal_short_id());
        for desc_id in &descendants {
            // update child score
            self.entries.modify_by_id(desc_id, |e| {
                match op {
                    EntryOp::Remove => e.inner.sub_ancestor_weight(parent),
                    EntryOp::Add => e.inner.add_ancestor_weight(parent),
                };
                e.score = e.inner.as_score_key();
            });
        }
    }

    fn record_entry_edges(&mut self, entry: &TxEntry) {
        let tx_short_id: ProposalShortId = entry.proposal_short_id();
        let header_deps = entry.transaction().header_deps();
        let related_dep_out_points: Vec<_> = entry.related_dep_out_points().cloned().collect();
        let inputs = entry.transaction().input_pts_iter();

        // if input reference a in-pool output, connect it
        // otherwise, record input for conflict check
        for i in inputs {
            self.edges.insert_input(i.to_owned(), tx_short_id.clone());
        }

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

    fn record_entry_descendants(&mut self, entry: &TxEntry) {
        let tx_short_id: ProposalShortId = entry.proposal_short_id();
        let outputs = entry.transaction().output_pts();
        let mut children = HashSet::new();

        // collect children
        for o in outputs {
            if let Some(ids) = self.edges.get_deps_ref(&o).cloned() {
                children.extend(ids);
            }
            if let Some(id) = self.edges.get_input_ref(&o).cloned() {
                children.insert(id);
            }
        }
        // update children
        if !children.is_empty() {
            for child in &children {
                self.links.add_parent(child, tx_short_id.clone());
            }
            if let Some(links) = self.links.inner.get_mut(&tx_short_id) {
                links.children.extend(children);
            }
            self.update_descendants_index_key(entry, EntryOp::Add);
        }
        // update ancestor's index key for adding new entry
        self.update_ancestors_index_key(entry, EntryOp::Add);
    }

    /// Check ancestors and record for entry
    fn check_and_record_ancestors(&mut self, entry: &mut TxEntry) -> Result<bool, Reject> {
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
            .calc_relation_ids(parents.clone(), Relation::Parents);

        // update parents references
        for ancestor_id in &ancestors {
            let ancestor = self.get_by_id_checked(ancestor_id);
            entry.add_ancestor_weight(&ancestor.inner);
        }
        if entry.ancestors_count > self.max_ancestors_count {
            return Err(Reject::ExceededMaximumAncestorsCount);
        }

        for parent in &parents {
            self.links.add_child(parent, short_id.clone());
        }

        let links = TxLinks {
            parents,
            children: Default::default(),
        };
        self.links.inner.insert(short_id, links);

        Ok(true)
    }

    fn remove_entry_edges(&mut self, entry: &TxEntry) {
        let inputs = entry.transaction().input_pts_iter();
        for i in inputs {
            // release input record
            self.edges.remove_input(&i);
        }

        let id = entry.proposal_short_id();
        for d in entry.related_dep_out_points().cloned() {
            self.edges.delete_txid_by_dep(d, &id);
        }

        self.edges.header_deps.remove(&id);
    }

    fn insert_entry(&mut self, entry: &TxEntry, status: Status) {
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
    }
}
