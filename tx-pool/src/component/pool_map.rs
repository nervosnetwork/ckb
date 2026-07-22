//! Top-level Pool type, methods, and tests
extern crate rustc_hash;
extern crate slab;
use super::links::TxLinks;
use crate::TxEntry;
use crate::component::links::{Relation, TxLinksMap};
use crate::component::out_point_index::OutPointIndex;
use crate::component::saturating_counter::SaturatingCounter;
use crate::component::sort_key::{AncestorsScoreSortKey, EvictKey};
use crate::error::Reject;
use ckb_logger::{debug, error, trace};
use ckb_types::core::error::OutPointError;
use ckb_types::core::{Cycle, FeeRate};
use ckb_types::packed::OutPoint;
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

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::Pending => write!(f, "pending"),
            Status::Gap => write!(f, "gap"),
            Status::Proposed => write!(f, "proposed"),
        }
    }
}

/// The result of [`PoolMap::conflict_closure`].
pub(crate) enum ConflictClosure {
    /// The union did not exceed the limit: the complete closure, with the
    /// post-ordered removal plan and the membership set.
    Complete {
        removal: Vec<ProposalShortId>,
        removal_set: HashSet<ProposalShortId>,
    },
    /// The union exceeded the limit and the traversal aborted early, so the
    /// caller's cap is the hard bound on traversal cost regardless of pool
    /// population. Carries the exact number of unique entries discovered
    /// when the traversal stopped (limit + 1).
    Exceeded { count_lower_bound: usize },
}

#[derive(Copy, Clone)]
enum EntryOp {
    Add,
    Remove,
}

#[derive(MultiIndexMap, Clone)]
pub struct PoolEntry {
    #[multi_index(hashed_unique)]
    pub(crate) id: ProposalShortId,
    #[multi_index(ordered_non_unique)]
    pub(crate) score: AncestorsScoreSortKey,
    #[multi_index(hashed_non_unique)]
    pub(crate) status: Status,
    #[multi_index(ordered_non_unique)]
    pub(crate) evict_key: EvictKey,
    // other sort key
    pub(crate) inner: TxEntry,
}

/// Aggregated statistics tracked by [`PoolMap`].
#[derive(Default)]
pub struct PoolStats {
    // sum of all tx_pool tx's virtual sizes.
    pub total_tx_size: SaturatingCounter<usize>,
    // sum of all tx_pool tx's cycles.
    pub total_tx_cycles: SaturatingCounter<Cycle>,
    pub pending_count: usize,
    pub gap_count: usize,
    pub proposed_count: usize,
}

impl PoolStats {
    pub fn clear(&mut self) {
        self.total_tx_size.set(0);
        self.total_tx_cycles.set(0);
        self.pending_count = 0;
        self.gap_count = 0;
        self.proposed_count = 0;
    }

    fn adjust_status_count(&mut self, remove: Option<Status>, add: Option<Status>) {
        if let Some(status) = remove {
            match status {
                Status::Pending => self.pending_count = self.pending_count.saturating_sub(1),
                Status::Gap => self.gap_count = self.gap_count.saturating_sub(1),
                Status::Proposed => self.proposed_count = self.proposed_count.saturating_sub(1),
            }
        }
        if let Some(status) = add {
            match status {
                Status::Pending => self.pending_count += 1,
                Status::Gap => self.gap_count += 1,
                Status::Proposed => self.proposed_count += 1,
            }
        }
    }

    /// Atomically adjust cached total size and cycles for a removed tx.
    /// If either counter underflows, use `recompute` (precomputed by the caller)
    /// to recover both counters together and stay consistent.
    fn adjust_totals(
        &mut self,
        tx_size: usize,
        cycles: Cycle,
        recompute: Option<(usize, Cycle)>,
        action: &'static str,
    ) {
        match (
            self.total_tx_size.get().checked_sub(tx_size),
            self.total_tx_cycles.get().checked_sub(cycles),
        ) {
            (Some(size), Some(cycles)) => {
                self.total_tx_size.set(size);
                self.total_tx_cycles.set(cycles);
            }
            _ => match recompute {
                Some((size, cycles)) => {
                    error!(
                        "tx-pool total stats underflowed when removing size {} cycles {} in {}, recomputed size {} cycles {}",
                        tx_size, cycles, action, size, cycles
                    );
                    self.total_tx_size.set(size);
                    self.total_tx_cycles.set(cycles);
                }
                None => {
                    error!(
                        "tx-pool total stats underflowed when removing size {} cycles {} in {}, and recomputing overflowed",
                        tx_size, cycles, action
                    );
                }
            },
        }
    }
}

pub struct PoolMap {
    /// The pool entries with different kinds of sort strategies
    pub(crate) entries: MultiIndexPoolEntryMap,
    /// All the deps, header_deps, inputs, outputs relationships
    pub(crate) out_point_index: OutPointIndex,
    /// All the parent/children relationships
    pub(crate) links: TxLinksMap,
    pub(crate) max_ancestors_count: usize,
    pub(crate) stats: PoolStats,
    /// Journal of entries evicted by the cell-ref escape hatch during the
    /// most recent `add_entry` call. Cleared at the start of every
    /// `add_entry`; the caller drains it on *both* outcomes, because
    /// `add_entry` can still fail after the escape eviction (e.g. the dep
    /// pre-validation) and would otherwise drop the evicted set on the
    /// error path — a failed commit must not evict in-pool transactions.
    pub(crate) evicted_journal: HashSet<TxEntry>,
}

impl PoolMap {
    pub fn new(max_ancestors_count: usize) -> Self {
        PoolMap {
            entries: MultiIndexPoolEntryMap::default(),
            out_point_index: OutPointIndex::default(),
            links: TxLinksMap::new(),
            max_ancestors_count,
            stats: PoolStats::default(),
            evicted_journal: HashSet::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn header_deps_len(&self) -> usize {
        self.out_point_index.header_deps_len()
    }

    #[cfg(test)]
    pub(crate) fn deps_len(&self) -> usize {
        self.out_point_index.deps_len()
    }

    #[cfg(test)]
    pub(crate) fn inputs_len(&self) -> usize {
        self.out_point_index.inputs_len()
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

    pub(crate) fn pending_size(&self) -> usize {
        self.stats.pending_count + self.stats.gap_count
    }

    pub(crate) fn proposed_size(&self) -> usize {
        self.stats.proposed_count
    }

    pub(crate) fn sorted_proposed_iter(&self) -> impl Iterator<Item = &TxEntry> {
        self.score_sorted_iter_by_status(Status::Proposed)
    }

    /// Iterate over pending and gap entries sorted by score.
    pub(crate) fn pending_gap_entries(&self) -> impl Iterator<Item = &TxEntry> {
        self.score_sorted_iter_by_statuses(vec![Status::Pending, Status::Gap])
    }

    /// Iterate over proposed entries sorted by score.
    pub(crate) fn proposed_entries(&self) -> impl Iterator<Item = &TxEntry> {
        self.sorted_proposed_iter()
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
                    .output_with_data(out_point.index().into())
            })
    }

    /// Insert a `TxEntry` into pool_map.
    ///
    /// ## Returns
    ///
    /// Returns `Reject` when any error happened, otherwise return `Ok((succ, evicts))`
    /// - succ  : means whether the entry is inserted actually into pool,
    /// - evicts: is the evicted transactions before inserting this `TxEntry`,
    ///   Currently, evicts when inserting is only due to referring cell dep will be consumed by this new transaction.
    pub(crate) fn add_entry(
        &mut self,
        mut entry: TxEntry,
        status: Status,
    ) -> Result<(bool, HashSet<TxEntry>), Reject> {
        // The journal belongs to this call: clear it before *any* exit,
        // including the duplicate early return below, so a stale journal
        // left by a previous caller (e.g. a successful escape-hatch
        // eviction that nobody drained) can never be picked up by this
        // call's caller and misattributed to this entry.
        self.evicted_journal.clear();

        let tx_short_id = entry.proposal_short_id();
        if self.entries.get_by_id(&tx_short_id).is_some() {
            return Ok((false, Default::default()));
        }
        trace!("pool_map.add_{:?} {}", status, entry.transaction().hash());

        // Order matters here. `check_and_prepare_ancestors` may *evict* pool
        // entries, and eviction can free cell-dep conflicts (an evicted
        // cell_ref_parent may have been consuming one of this entry's deps).
        // So the sequence is:
        //   1. pre-validate inputs (defensive: input conflicts are always
        //      resolved before `add_entry`, and eviction never occupies new
        //      inputs, so an early check is safe);
        //   2. defensive overflow check for the stat counters (read-only; the
        //      actual deltas are applied after eviction, because eviction
        //      itself decrements the counters);
        //   3. evict/check ancestors and apply ancestor weights to `entry`
        //      (may free dep conflicts; does not touch the link graph);
        //   4. pre-validate deps (must be after eviction);
        //   5. commit ancestor links, then mutate freely.
        // Any failure happens before the mutating steps, so the evicted set
        // can never be lost on the `Err` path, and the link graph is only
        // written at step 5, so a rejected entry leaves no ghost nodes
        // behind. Escape-hatch evictions are journaled (see
        // `evicted_journal`) so the caller can recover them even when a
        // later step rejects the entry.
        self.pre_validate_entry_inputs(&entry)?;
        self.update_stat_for_add_tx(entry.size, entry.cycles)?;

        let (evicts, parents) = self.check_and_prepare_ancestors(&mut entry)?;
        self.pre_validate_entry_deps(&entry)?;
        self.commit_ancestor_links(tx_short_id, parents);

        self.record_entry_edges(&entry);
        // Link children that entered the pool *before* this entry and fold
        // their weight into the entry's own descendant statistics — before
        // `insert_entry` freezes the derived keys, so the evict key already
        // reflects the children.
        self.link_and_fold_children(&mut entry);
        self.insert_entry(&entry, status);
        // Update the derived keys on both sides: the children's ancestor
        // side and the ancestors' descendant side. The entry's own
        // ancestor/descendant weights are already folded into `entry`.
        self.update_descendants_index_key(&entry, EntryOp::Add);
        self.update_ancestors_index_key(&entry, EntryOp::Add);
        self.track_entry_statistics(None, Some(status));
        // Apply the stat deltas *after* eviction: applying values computed
        // before it would clobber the decrements `update_stat_for_remove_tx`
        // made for the evicted entries. The overflow case was already
        // rejected by the check above, so `add_saturating` cannot actually
        // overflow here.
        self.stats
            .total_tx_size
            .add_saturating(entry.size, "pool_map total_tx_size", "add_entry");
        self.stats.total_tx_cycles.add_saturating(
            entry.cycles,
            "pool_map total_tx_cycles",
            "add_entry",
        );
        Ok((true, evicts))
    }

    /// Defensive read-only check: none of the entry's inputs is already
    /// consumed by another in-pool transaction.
    ///
    /// Input conflicts are always resolved before `add_entry` (rejected by
    /// `check_rtx` or removed by `process_rbf`), so this should never fail;
    /// between this check and `record_entry_edges` the only mutation is
    /// ancestor eviction, which frees inputs but never occupies new ones.
    fn pre_validate_entry_inputs(&self, entry: &TxEntry) -> Result<(), Reject> {
        for i in entry.transaction().input_pts_iter() {
            if let Some(conflict) = self.out_point_index.get_input_ref(&i) {
                debug!(
                    "pre_validate_entry_inputs: input {:?} already consumed by {}",
                    i, conflict
                );
                return Err(Reject::Resolve(OutPointError::Dead(i)));
            }
        }
        Ok(())
    }

    /// Read-only check that none of the entry's cell-deps is consumed by
    /// another in-pool transaction (deps that are also inputs of this same tx
    /// are exempt).
    ///
    /// Must run *after* `check_and_record_ancestors`: evicting a
    /// cell_ref_parent can free a dep conflict, and rejecting before eviction
    /// would turn a previously acceptable transaction away.
    fn pre_validate_entry_deps(&self, entry: &TxEntry) -> Result<(), Reject> {
        let inputs: HashSet<OutPoint> = entry.transaction().input_pts_iter().collect();
        for d in entry.related_dep_out_points() {
            if inputs.contains(d) {
                continue;
            }
            if self.out_point_index.get_input_ref(d).is_some() {
                return Err(Reject::Resolve(OutPointError::Dead(d.clone())));
            }
        }
        Ok(())
    }

    /// Change the status of the entry; used by `pending_rtx` / `gap_rtx` /
    /// `proposed_rtx` during mine-mode proposal-window reconciliation.
    pub(crate) fn set_entry(&mut self, short_id: &ProposalShortId, status: Status) {
        let mut old_status = None;
        self.entries
            .modify_by_id(short_id, |e| {
                old_status = Some(e.status);
                e.status = status;
            })
            .expect("inconsistent pool");
        self.track_entry_statistics(old_status, Some(status));
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
            self.track_entry_statistics(Some(entry.status), None);
            self.update_stat_for_remove_tx(entry.inner.size, entry.inner.cycles);
            entry.inner
        })
    }

    pub(crate) fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        let mut removed_ids = vec![id.to_owned()];
        removed_ids.extend(self.calc_descendants(id));

        // Remove entries in post-order (children before parents) so that
        // update_ancestors_index_key / update_descendants_index_key still see
        // valid links while subtracting weights. Removing links upfront would
        // leave surviving entries with stale ancestor/descendant statistics.
        let removed_set: HashSet<ProposalShortId> = removed_ids.iter().cloned().collect();
        let mut ordered = Vec::with_capacity(removed_ids.len());
        let mut visited = HashSet::with_capacity(removed_ids.len());
        // Iterative DFS to avoid stack overflow for deeply nested descendant
        // chains.  Each stack frame is (id, children_already_processed).
        let mut stack: Vec<(&ProposalShortId, bool)> =
            removed_ids.iter().map(|id| (id, false)).collect();
        while let Some((id, processed)) = stack.pop() {
            if !removed_set.contains(id) {
                continue;
            }
            if processed {
                ordered.push(id.clone());
                continue;
            }
            if !visited.insert(id.clone()) {
                continue;
            }
            stack.push((id, true));
            if let Some(children) = self.links.get_children(id) {
                for child in children {
                    stack.push((child, false));
                }
            }
        }

        ordered
            .iter()
            .filter_map(|id| self.remove_entry(id))
            .collect()
    }

    /// Compute the conflict closure of `roots` in a single multi-source
    /// traversal that both collects the set and emits it post-ordered
    /// (children before parents, as `remove_entry` requires for index
    /// weights to be subtracted against still-valid links).
    ///
    /// The traversal aborts as soon as the union exceeds `limit` unique
    /// entries, so the caller's cap (RBF rule #5's
    /// `MAX_RBF_REPLACEMENT_CANDIDATES`) is the hard bound on cost regardless
    /// of pool population. Earlier versions first computed every root's
    /// descendants separately (shared subtrees walked once per root) and
    /// then re-walked the union for ordering.
    pub(crate) fn conflict_closure(
        &self,
        roots: &HashSet<ProposalShortId>,
        limit: usize,
    ) -> ConflictClosure {
        let mut removal_set: HashSet<ProposalShortId> = HashSet::new();
        let mut ordered = Vec::new();
        let mut visited: HashSet<ProposalShortId> = HashSet::new();
        let mut stack: Vec<(ProposalShortId, bool)> =
            roots.iter().cloned().map(|id| (id, false)).collect();

        while let Some((id, processed)) = stack.pop() {
            if processed {
                ordered.push(id);
                continue;
            }
            if !visited.insert(id.clone()) {
                continue;
            }
            // A links node with no matching entry is a ghost: it still
            // participates in the traversal (its children may be real
            // descendants) and in the removal plan (skipped there because
            // `remove_entry` returns `None` for it), but it must never
            // count against the caller's limit.
            if self.entries.get_by_id(&id).is_some() {
                removal_set.insert(id.clone());
                if removal_set.len() > limit {
                    return ConflictClosure::Exceeded {
                        count_lower_bound: removal_set.len(),
                    };
                }
            }
            stack.push((id.clone(), true));
            if let Some(children) = self.links.get_children(&id) {
                for child in children {
                    stack.push((child.clone(), false));
                }
            }
        }

        ConflictClosure::Complete {
            removal: ordered,
            removal_set,
        }
    }

    pub(crate) fn resolve_conflict_header_dep(
        &mut self,
        headers: &HashSet<Byte32>,
    ) -> Vec<ConflictEntry> {
        let mut conflicts = Vec::new();

        // invalid header deps
        let mut ids = Vec::new();
        for (tx_id, deps) in self.out_point_index.header_deps.iter() {
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
            .filter_map(|out_point| self.out_point_index.get_input_ref(&out_point).cloned())
            .collect()
    }

    pub(crate) fn find_conflict_outpoint(&self, tx: &TransactionView) -> Option<OutPoint> {
        tx.input_pts_iter().find_map(|out_point| {
            self.out_point_index
                .get_input_ref(&out_point)
                .map(|_| out_point)
        })
    }

    pub(crate) fn resolve_conflict(&mut self, tx: &TransactionView) -> Vec<ConflictEntry> {
        let mut conflicts = Vec::new();

        for i in tx.input_pts_iter() {
            if let Some(id) = self.out_point_index.remove_input(&i) {
                let entries = self.remove_entry_and_descendants(&id);
                if !entries.is_empty() {
                    let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                    let rejects = std::iter::repeat_n(reject, entries.len());
                    conflicts.extend(entries.into_iter().zip(rejects));
                }
            }

            // deps consumed
            if let Some(x) = self.out_point_index.remove_deps(&i) {
                for id in x {
                    let entries = self.remove_entry_and_descendants(&id);
                    if !entries.is_empty() {
                        let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                        let rejects = std::iter::repeat_n(reject, entries.len());
                        conflicts.extend(entries.into_iter().zip(rejects));
                    }
                }
            }
        }

        conflicts
    }

    pub(crate) fn estimate_fee_rate(
        &self,
        mut target_blocks: usize,
        max_block_bytes: usize,
        max_block_cycles: Cycle,
        min_fee_rate: FeeRate,
    ) -> FeeRate {
        debug_assert!(target_blocks > 0);
        let iter = self.entries.iter_by_score().rev();
        let mut current_block_bytes = 0;
        let mut current_block_cycles = 0;
        for entry in iter {
            current_block_bytes += entry.inner.size;
            current_block_cycles += entry.inner.cycles;
            if current_block_bytes >= max_block_bytes || current_block_cycles >= max_block_cycles {
                target_blocks -= 1;
                if target_blocks == 0 {
                    return entry.inner.fee_rate();
                }
                current_block_bytes = entry.inner.size;
                current_block_cycles = entry.inner.cycles;
            }
        }

        min_fee_rate
    }

    // find the pending txs sorted by score, and return their proposal short ids
    #[cfg(test)]
    pub(crate) fn get_proposals(
        &self,
        limit: usize,
        exclusion: &HashSet<ProposalShortId>,
    ) -> HashSet<ProposalShortId> {
        self.score_sorted_iter_by_status(Status::Pending)
            .filter_map(|entry| {
                let id = entry.proposal_short_id();
                (!exclusion.contains(&id)).then_some(id)
            })
            .take(limit)
            .collect()
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
        self.out_point_index.clear();
        self.links.clear();
        self.stats.clear();
    }

    pub(crate) fn score_sorted_iter_by_status(
        &self,
        status: Status,
    ) -> impl Iterator<Item = &TxEntry> {
        self.entries
            .iter_by_score()
            .rev()
            .filter_map(move |entry| (entry.status == status).then_some(&entry.inner))
    }

    pub(crate) fn score_sorted_iter_by_statuses(
        &self,
        statuses: Vec<Status>,
    ) -> impl Iterator<Item = &TxEntry> {
        self.entries
            .iter_by_score()
            .rev()
            .filter_map(move |entry| statuses.contains(&entry.status).then_some(&entry.inner))
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
        let inputs: HashSet<OutPoint> = entry.transaction().input_pts_iter().collect();

        // if input reference a in-pool output, connect it
        // otherwise, record input for conflict check
        //
        // Cannot fail: `add_entry` ran `pre_validate_entry_inputs` /
        // `pre_validate_entry_deps` before this point, and the only mutation
        // in between (ancestor eviction) frees inputs rather than occupying
        // them.
        for i in &inputs {
            self.out_point_index
                .insert_input(i.to_owned(), tx_short_id.clone())
                .expect("entry inputs pre-validated as unoccupied");
        }

        // record dep-txid
        for d in related_dep_out_points {
            // CKB allows a transaction to reference the same out-point both as
            // an input and as a cell dep. Such a dep does not represent a
            // dependency on another tx; it is consumed by this tx itself, so
            // skip recording it and skip the in-pool input conflict check.
            if inputs.contains(&d) {
                continue;
            }
            debug_assert!(self.out_point_index.get_input_ref(&d).is_none());
            self.out_point_index.insert_deps(d, tx_short_id.clone());
        }
        // record header_deps
        if !header_deps.is_empty() {
            self.out_point_index
                .header_deps
                .insert(tx_short_id, header_deps.into_iter().collect());
        }
    }

    /// Link an entry to children that entered the pool *before* it (they
    /// spend or cell-dep on its outputs) and fold their weight into the
    /// entry's own descendant statistics.
    ///
    /// Must run after `commit_ancestor_links` (the entry's links node must
    /// exist) and before `insert_entry` (the derived keys are frozen at
    /// insert time). The weight fold is the symmetric half of
    /// `update_ancestors_index_key(..., Add)` (which updates the ancestors'
    /// descendant side): without it, the entry's own `descendants_*` stay
    /// at their self-only initial values, and the day such a child leaves,
    /// `sub_descendant_weight` saturates them down to zero — permanently
    /// corrupting the entry's evict key (CPFP protection and eviction
    /// order).
    fn link_and_fold_children(&mut self, entry: &mut TxEntry) {
        let tx_short_id = entry.proposal_short_id();
        let outputs = entry.transaction().output_pts();
        let mut children = HashSet::with_capacity(outputs.len());

        // collect children
        for o in outputs {
            if let Some(ids) = self.out_point_index.get_deps_ref(&o).cloned() {
                children.extend(ids);
            }
            if let Some(id) = self.out_point_index.get_input_ref(&o).cloned() {
                children.insert(id);
            }
        }
        if children.is_empty() {
            return;
        }

        let mut delta = crate::component::entry::WeightDelta::default();
        for child in &children {
            if let Some(child_entry) = self.get_by_id(child) {
                delta.add_entry(&child_entry.inner);
            } else {
                error!(
                    "tx-pool: out_point_index references missing child entry {}",
                    child
                );
            }
            self.links.add_parent(child, tx_short_id.clone());
        }
        if let Some(links) = self.links.get_mut(&tx_short_id) {
            links.children.extend(children);
        }
        entry.add_descendants_weight(delta);
    }

    // return (ancestors, parents, cell_ref_parents)
    // `cell_ref_parents` may be invalidate when the tx consuming the cell is submitted
    fn get_tx_ancestors(
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
            if let Some(deps) = self.out_point_index.deps.get(&input_pt) {
                cell_ref_parents.extend(deps.iter().cloned());
                parents.extend(deps.iter().cloned());
            }

            let id = ProposalShortId::from_tx_hash(&input_pt.tx_hash());
            if self.links.contains_key(&id) {
                parents.insert(id);
            }
        }
        for cell_dep in entry.cell_deps() {
            let dep_pt = cell_dep.out_point();
            let id = ProposalShortId::from_tx_hash(&dep_pt.tx_hash());
            if self.links.contains_key(&id) {
                parents.insert(id);
            }
        }

        let ancestors = self
            .links
            .calc_relation_ids(parents.clone(), Relation::Parents);

        (ancestors, parents, cell_ref_parents)
    }

    /// Fold every ancestor's weight into `entry`. Read-only with respect to
    /// the pool: it must stay safe to call before all fallible validations
    /// have run, so it never touches `self.links` (see
    /// [`Self::commit_ancestor_links`]).
    fn apply_ancestor_weights(
        &self,
        entry: &mut TxEntry,
        ancestors: &HashSet<ProposalShortId>,
    ) -> Result<(), Reject> {
        for ancestor_id in ancestors {
            let ancestor = self.get_by_id(ancestor_id).ok_or_else(|| {
                error!(
                    "tx-pool internal invariant broken: missing entry for ancestor {}",
                    ancestor_id
                );
                Reject::Malformed(
                    "pool".to_string(),
                    format!("inconsistent pool: missing entry for {}", ancestor_id),
                )
            })?;
            entry.add_ancestor_weight(&ancestor.inner);
        }
        Ok(())
    }

    /// Commit the ancestor links for an entry that has passed every fallible
    /// validation. Infallible.
    ///
    /// Must only be called from `add_entry` *after* `pre_validate_entry_deps`:
    /// writing the link node (and the parent→child references) any earlier
    /// would leave a ghost node behind on the error path — an id present in
    /// `links` but absent from `entries`, poisoning descendant counts
    /// (`calc_descendants`, `validate_ancestor_capacity`) for a transaction
    /// that never entered the pool.
    fn commit_ancestor_links(
        &mut self,
        short_id: ProposalShortId,
        parents: HashSet<ProposalShortId>,
    ) {
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

    /// Read-only pre-validation of `check_and_record_ancestors`' failure
    /// condition, evaluated as if the `excluded` entries had already been
    /// removed from the pool.
    ///
    /// RBF replacements remove their conflicts (and the conflicts'
    /// descendants) before the new entry is committed. When committing the
    /// new entry is *certain* to hit the ancestor-count limit even after
    /// that removal, the removal must not happen at all: otherwise an
    /// attacker can repeat remove-then-restore of the whole victim cluster
    /// on every attempt with replacements that never had a chance to
    /// commit, at no cost (a failed replacement pays no fee).
    ///
    /// The check mirrors `check_and_record_ancestors` exactly, including
    /// the cell-ref eviction escape hatch: failure is only certain when
    /// evicting every surviving `cell_ref_parent` still cannot bring the
    /// count under the limit. Borderline cases the approximate eviction
    /// loop might handle differently fall through to the caller's normal
    /// remove-and-recover path, so this pre-validation is conservative and
    /// can never reject a committable entry.
    pub(crate) fn validate_ancestor_capacity(
        &self,
        tx: &TransactionView,
        excluded: &HashSet<ProposalShortId>,
    ) -> Result<(), Reject> {
        let (ancestors, _parents, cell_ref_parents) = self.get_tx_ancestors(tx);
        let effective: HashSet<&ProposalShortId> = ancestors.difference(excluded).collect();
        let evictable = cell_ref_parents
            .iter()
            .filter(|id| effective.contains(id))
            .count();
        // Mirrors `check_and_record_ancestors`: it fails when
        // `ancestors_count - cell_ref_parents.len() > max_ancestors_count`,
        // where `ancestors_count` is `effective.len() + 1` once the removed
        // entries are gone.
        if effective.len() + 1 > self.max_ancestors_count + evictable {
            return Err(Reject::ExceededMaximumAncestorsCount);
        }
        Ok(())
    }

    /// Check ancestors and compute the link plan for `entry`.
    ///
    /// Returns the entries evicted by the cell-ref escape hatch and the final
    /// parent set to link. Applies ancestor weights to `entry` but does *not*
    /// touch `self.links`: the caller commits the links via
    /// [`Self::commit_ancestor_links`] only after every fallible validation
    /// has passed, so a rejected entry never leaves ghost nodes behind.
    ///
    /// This can fail with `ExceededMaximumAncestorsCount` *after* an RBF
    /// replacement has already removed its conflicts. That failure is
    /// pre-validated in `prepare_rbf_replacement` via
    /// [`Self::validate_ancestor_capacity`] before any removal, so the
    /// remove-and-recover rollback is only needed for cases the
    /// pre-validation cannot decide. Note the ancestry can be arbitrarily
    /// long even under the RBF rules: rule #2 only restricts the
    /// replacement's *inputs*, not its cell deps, so "a replacement cannot
    /// be in a long transaction chain" is not a safe assumption.
    fn check_and_prepare_ancestors(
        &mut self,
        entry: &mut TxEntry,
    ) -> Result<(HashSet<TxEntry>, HashSet<ProposalShortId>), Reject> {
        let tx = entry.transaction();
        let (ancestors, mut parents, cell_ref_parents) = self.get_tx_ancestors(tx);

        let mut ancestors_count = ancestors.len() + 1;
        let mut evicted = Default::default();

        if ancestors_count <= self.max_ancestors_count {
            self.apply_ancestor_weights(entry, &ancestors)?;
            return Ok((evicted, parents));
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
                    // The cascade removes `next_id` *and its descendants*,
                    // and any of them may be a direct parent of the new
                    // entry. Every removed id must leave the parent set: a
                    // leftover id is a ghost that `calc_relation_ids`
                    // recounts unconditionally (it never checks that the id
                    // is still linked), and the weight fold below would
                    // then fail with a spurious "missing entry" Malformed.
                    for removed_entry in &removed {
                        parents.remove(&removed_entry.proposal_short_id());
                    }
                    // Journal the escape-hatch evictions so the caller can
                    // recover them if a later step rejects this entry.
                    self.evicted_journal.extend(removed.iter().cloned());
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

        // The recount should be under the limit now. This is a defensive
        // error, not an `assert!`: the function runs inside the tx-pool
        // write lock, and a panic would unwind past the `evicted_journal`
        // recovery protocol, permanently losing the evicted transactions.
        if ancestors.len() >= self.max_ancestors_count {
            error!(
                "tx-pool escape-hatch eviction left {} ancestors (max {}), rejecting defensively",
                ancestors.len(),
                self.max_ancestors_count
            );
            return Err(Reject::ExceededMaximumAncestorsCount);
        }

        self.apply_ancestor_weights(entry, &ancestors)?;
        Ok((evicted, parents))
    }

    fn remove_entry_edges(&mut self, entry: &TxEntry) {
        for i in entry.transaction().input_pts_iter() {
            // release input record
            self.out_point_index.remove_input(&i);
        }
        let id = entry.proposal_short_id();
        for d in entry.related_dep_out_points().cloned() {
            self.out_point_index.delete_txid_by_dep(d, &id);
        }

        self.out_point_index.header_deps.remove(&id);
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

    fn track_entry_statistics(&mut self, remove: Option<Status>, add: Option<Status>) {
        self.stats.adjust_status_count(remove, add);
        debug_assert_eq!(
            self.stats.pending_count + self.stats.gap_count + self.stats.proposed_count,
            self.entries.len()
        );
        if let Some(metrics) = ckb_metrics::handle() {
            metrics
                .ckb_tx_pool_entry
                .pending
                .set(self.stats.pending_count as i64);
            metrics
                .ckb_tx_pool_entry
                .gap
                .set(self.stats.gap_count as i64);
            metrics
                .ckb_tx_pool_entry
                .proposed
                .set(self.stats.proposed_count as i64);
        }
    }

    fn recompute_total_stat(&self) -> Option<(usize, Cycle)> {
        self.entries.iter().try_fold(
            (0usize, 0 as Cycle),
            |(total_size, total_cycles), (_, entry)| {
                Some((
                    total_size.checked_add(entry.inner.size)?,
                    total_cycles.checked_add(entry.inner.cycles)?,
                ))
            },
        )
    }

    /// Calculate size and cycles statistics for adding a tx.
    fn update_stat_for_add_tx(
        &self,
        tx_size: usize,
        cycles: Cycle,
    ) -> Result<(usize, Cycle), Reject> {
        let total_tx_size = self
            .stats
            .total_tx_size
            .get()
            .checked_add(tx_size)
            .ok_or_else(|| {
                Reject::Full(format!(
                    "tx-pool total_tx_size {} overflows by add {}",
                    self.stats.total_tx_size.get(),
                    tx_size
                ))
            })?;
        let total_tx_cycles = self
            .stats
            .total_tx_cycles
            .get()
            .checked_add(cycles)
            .ok_or_else(|| {
                Reject::Full(format!(
                    "tx-pool total_tx_cycles {} overflows by add {}",
                    self.stats.total_tx_cycles.get(),
                    cycles
                ))
            })?;
        Ok((total_tx_size, total_tx_cycles))
    }

    /// Update size and cycles statistics for remove tx.
    /// Cycles overflow is possible because cycle counts are not always accurate.
    fn update_stat_for_remove_tx(&mut self, tx_size: usize, cycles: Cycle) {
        let needs_recompute = self
            .stats
            .total_tx_size
            .get()
            .checked_sub(tx_size)
            .is_none()
            || self
                .stats
                .total_tx_cycles
                .get()
                .checked_sub(cycles)
                .is_none();
        let recompute = if needs_recompute {
            self.recompute_total_stat()
        } else {
            None
        };
        self.stats
            .adjust_totals(tx_size, cycles, recompute, "remove_tx");
    }
}
