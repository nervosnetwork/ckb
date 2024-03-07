//! Top-level Pool type, methods, and tests
extern crate rustc_hash;
extern crate slab;
use super::links::TxLinks;
use crate::component::edges::Edges;
use crate::component::links::{Relation, TxLinksMap};
use crate::component::sort_key::{AncestorsScoreSortKey, EvictKey};
use crate::error::Reject;
use crate::TxEntry;
use ckb_logger::{debug, error, trace};
use ckb_types::core::error::OutPointError;
use ckb_types::core::Cycle;
use ckb_types::packed::OutPoint;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::TransactionView,
    packed::{Byte32, CellOutput, ProposalShortId},
};
use multi_index_map::MultiIndexMap;
use std::collections::HashSet;

type ConflictEntry = (TxEntry, Reject);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    Pending,
    Gap,
    Proposed,
}

impl ToString for Status {
    fn to_string(&self) -> String {
        match self {
            Status::Pending => "pending".to_string(),
            Status::Gap => "gap".to_string(),
            Status::Proposed => "proposed".to_string(),
        }
    }
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
    #[multi_index(hashed_non_unique)]
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
    // sum of all tx_pool tx's virtual sizes.
    pub(crate) total_tx_size: usize,
    // sum of all tx_pool tx's cycles.
    pub(crate) total_tx_cycles: Cycle,
}

impl PoolMap {
    pub fn new(max_ancestors_count: usize) -> Self {
        PoolMap {
            entries: MultiIndexPoolEntryMap::default(),
            edges: Edges::default(),
            links: TxLinksMap::new(),
            max_ancestors_count,
            total_tx_size: 0,
            total_tx_cycles: 0,
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
            .map(|(succ, _)| succ)
    }

    pub(crate) fn get_max_update_time(&self) -> u64 {
        self.entries
            .iter()
            .map(|(_, entry)| entry.inner.timestamp)
            .max()
            .unwrap_or(0)
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

    /// Inesrt a `TxEntry` into pool_map.
    ///
    /// ## Returns
    ///
    /// Returns `Reject` when any error happened, otherwise return `Ok((succ, evicts))`
    /// - succ  : means whether the entry is insertted actually into pool,
    /// - evicts: is the evicted transactions before inserting this `TxEntry`,
    ///           Currently, evicts when inserting is only due to reffering cell dep will be consumed by this new transaction.
    pub(crate) fn add_entry(
        &mut self,
        mut entry: TxEntry,
        status: Status,
    ) -> Result<(bool, HashSet<TxEntry>), Reject> {
        let tx_short_id = entry.proposal_short_id();
        let mut evicts = Default::default();
        if self.entries.get_by_id(&tx_short_id).is_some() {
            return Ok((false, evicts));
        }
        trace!("pool_map.add_{:?} {}", status, entry.transaction().hash());
        evicts = self.check_and_record_ancestors(&mut entry)?;
        self.record_entry_edges(&entry)?;
        self.insert_entry(&entry, status);
        self.record_entry_descendants(&entry);
        self.track_entry_statics();
        self.update_stat_for_add_tx(entry.size, entry.cycles);
        Ok((true, evicts))
    }

    /// Change the status of the entry, only used for `gap_rtx` and `proposed_rtx`
    pub(crate) fn set_entry(&mut self, short_id: &ProposalShortId, status: Status) {
        self.entries
            .modify_by_id(short_id, |e| {
                e.status = status;
            })
            .expect("unconsistent pool");
        self.track_entry_statics();
    }

    pub(crate) fn remove_entry(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        self.entries.remove_by_id(id).map(|entry| {
            debug!(
                "remove entry {} from status: {:?}",
                entry.inner.transaction().hash(),
                entry.status
            );
            self.update_ancestors_index_key(&entry.inner, EntryOp::Remove);
            self.update_descendants_index_key(&entry.inner, EntryOp::Remove);
            self.remove_entry_edges(&entry.inner);
            self.remove_entry_links(id);
            self.update_stat_for_remove_tx(entry.inner.size, entry.inner.cycles);
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
        tx.input_pts_iter()
            .filter_map(|out_point| self.edges.get_input_ref(&out_point).cloned())
            .collect()
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
        self.total_tx_size = 0;
        self.total_tx_cycles = 0;
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

    fn record_entry_edges(&mut self, entry: &TxEntry) -> Result<(), Reject> {
        let tx_short_id: ProposalShortId = entry.proposal_short_id();
        let header_deps = entry.transaction().header_deps();
        let related_dep_out_points: Vec<_> = entry.related_dep_out_points().cloned().collect();
        let inputs = entry.transaction().input_pts_iter();

        // if input reference a in-pool output, connect it
        // otherwise, record input for conflict check
        for i in inputs {
            self.edges.insert_input(i.to_owned(), tx_short_id.clone())?;
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
        Ok(())
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

    // return (ancestors, parents, cell_ref_parents)
    // `cell_ref_parents` may be invalidate when the tx consuming the cell is submitted
    fn get_tx_ancenstors(
        &self,
        entry: &TransactionView,
    ) -> (
        HashSet<ProposalShortId>,
        HashSet<ProposalShortId>,
        HashSet<ProposalShortId>,
    ) {
        let mut parents: HashSet<ProposalShortId> =
            HashSet::with_capacity(entry.inputs().len() + entry.cell_deps().len());
        let mut cell_ref_parents: HashSet<ProposalShortId> = Default::default();

        for input in entry.inputs() {
            let input_pt = input.previous_output();
            if let Some(deps) = self.edges.deps.get(&input_pt) {
                cell_ref_parents.extend(deps.iter().cloned());
                parents.extend(deps.iter().cloned());
            }

            let id = ProposalShortId::from_tx_hash(&input_pt.tx_hash());
            if self.links.inner.contains_key(&id) {
                parents.insert(id);
            }
        }
        for cell_dep in entry.cell_deps() {
            let dep_pt = cell_dep.out_point();
            let id = ProposalShortId::from_tx_hash(&dep_pt.tx_hash());
            if self.links.inner.contains_key(&id) {
                parents.insert(id);
            }
        }

        let ancestors = self
            .links
            .calc_relation_ids(parents.clone(), Relation::Parents);

        (ancestors, parents, cell_ref_parents)
    }

    fn _record_ancestors(
        &mut self,
        entry: &mut TxEntry,
        ancestors: HashSet<ProposalShortId>,
        parents: HashSet<ProposalShortId>,
    ) {
        // update parents references
        for ancestor_id in &ancestors {
            let ancestor = self.get_by_id_checked(ancestor_id);
            entry.add_ancestor_weight(&ancestor.inner);
        }

        let short_id = entry.proposal_short_id();

        for parent in &parents {
            self.links.add_child(parent, short_id.clone());
        }
        self.links.add_link(
            short_id,
            TxLinks {
                parents,
                children: Default::default(),
            },
        );
    }

    /// Check ancestors and record for entry
    // FIXME: In the scenario that a transaction passed all RBF rules, and then removed the conflicted
    // transaction in txpool, then failed with max ancestor limits, we now need to rollback the removing.
    // this is not an issue currently, because RBF have a rule that not allow any unknown inputs except
    // the conflicted inputs, so the new transcation can not be in a long transaction chain.
    // but it's still safer to report an error before any writing kind of operation.
    fn check_and_record_ancestors(
        &mut self,
        entry: &mut TxEntry,
    ) -> Result<HashSet<TxEntry>, Reject> {
        let tx = entry.transaction();
        let (ancestors, mut parents, cell_ref_parents) = self.get_tx_ancenstors(tx);

        let mut ancestors_count = ancestors.len() + 1;
        let mut evicted = Default::default();

        if ancestors_count <= self.max_ancestors_count {
            self._record_ancestors(entry, ancestors, parents);
            return Ok(evicted);
        }

        if ancestors_count.saturating_sub(cell_ref_parents.len()) <= self.max_ancestors_count {
            // if ancestors count exceed limitation,
            // try to evict some conflicted transactions due to ref cells

            // sort them to find out the transactions with lowest fees
            let evict_candidates: Vec<ProposalShortId> = self
                .entries
                .iter_by_evict_key()
                .filter(move |entry| cell_ref_parents.contains(&entry.id))
                .map(|x| x.id.clone())
                .collect();

            let mut iter = evict_candidates.iter();
            while ancestors_count > self.max_ancestors_count {
                if let Some(next_id) = iter.next() {
                    let removed = self.remove_entry_and_descendants(next_id);
                    ancestors_count = ancestors_count.saturating_sub(1);
                    parents.remove(next_id);
                    evicted.extend(removed);
                } else {
                    break;
                }
            }
        } else {
            return Err(Reject::ExceededMaximumAncestorsCount);
        }

        // some txs in `parents` are removed, now `ancestors` need to re-caculate,
        let ancestors = self
            .links
            .calc_relation_ids(parents.clone(), Relation::Parents);

        // we can assume the number now is less than `max_ancestors_count`
        assert!(ancestors.len() < self.max_ancestors_count);

        self._record_ancestors(entry, ancestors, parents);
        Ok(evicted)
    }

    fn remove_entry_edges(&mut self, entry: &TxEntry) {
        for i in entry.transaction().input_pts_iter() {
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

    fn track_entry_statics(&self) {
        if let Some(metrics) = ckb_metrics::handle() {
            metrics
                .ckb_tx_pool_entry
                .pending
                .set(self.entries.get_by_status(&Status::Pending).len() as i64);
            metrics
                .ckb_tx_pool_entry
                .gap
                .set(self.entries.get_by_status(&Status::Gap).len() as i64);
            metrics
                .ckb_tx_pool_entry
                .proposed
                .set(self.proposed_size() as i64);
        }
    }

    /// Update size and cycles statistics for add tx
    fn update_stat_for_add_tx(&mut self, tx_size: usize, cycles: Cycle) {
        let total_tx_size = self.total_tx_size.checked_add(tx_size).unwrap_or_else(|| {
            error!(
                "total_tx_size {} overflown by add {}",
                self.total_tx_size, tx_size
            );
            self.total_tx_size
        });
        let total_tx_cycles = self.total_tx_cycles.checked_add(cycles).unwrap_or_else(|| {
            error!(
                "total_tx_cycles {} overflown by add {}",
                self.total_tx_cycles, cycles
            );
            self.total_tx_cycles
        });
        self.total_tx_size = total_tx_size;
        self.total_tx_cycles = total_tx_cycles;
    }

    /// Update size and cycles statistics for remove tx
    /// cycles overflow is possible, currently obtaining cycles is not accurate
    fn update_stat_for_remove_tx(&mut self, tx_size: usize, cycles: Cycle) {
        let total_tx_size = self.total_tx_size.checked_sub(tx_size).unwrap_or_else(|| {
            error!(
                "total_tx_size {} overflown by sub {}",
                self.total_tx_size, tx_size
            );
            0
        });
        let total_tx_cycles = self.total_tx_cycles.checked_sub(cycles).unwrap_or_else(|| {
            error!(
                "total_tx_cycles {} overflown by sub {}",
                self.total_tx_cycles, cycles
            );
            0
        });
        self.total_tx_size = total_tx_size;
        self.total_tx_cycles = total_tx_cycles;
    }
}
