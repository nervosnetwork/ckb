//! Top-level Pool type, methods, and tests
extern crate rustc_hash;
extern crate slab;
#[cfg(test)]
#[path = "tests/pool_map_audit.rs"]
mod audit;
use super::links::TxLinks;
use crate::TxEntry;
use crate::component::links::{Relation, TxLinksMap};
use crate::component::out_point_index::OutPointIndex;
use crate::component::sort_key::{AncestorsScoreSortKey, EvictKey};
use crate::constants::MAX_POOL_MUTATION_CANDIDATES;
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

/// Exact undo record for a physical pool removal.
///
/// A failed commit must restore both the entry and its proposal-window
/// status. Reconstructing only from `TransactionView` loses
/// Pending/Gap/Proposed state and opens a window where competing commits can
/// observe the removed inputs as free.
#[derive(Debug, Clone)]
pub(crate) struct RemovedPoolEntry {
    pub(crate) entry: TxEntry,
    pub(crate) status: Status,
}

/// Complete result of one atomic PoolMap insertion.
///
/// Escape-hatch evictions are part of this value rather than a mutable
/// side-channel on `PoolMap`: a successful caller cannot forget to drain
/// hidden resident state, while every failed insertion restores its exact
/// pre-call membership before returning `Err`.
#[derive(Debug, Default)]
pub(crate) struct PoolMapAddOutcome {
    pub(crate) inserted: bool,
    pub(crate) evicted: Vec<RemovedPoolEntry>,
}

/// Parent roles for one verified incoming entry.
///
/// `required` producers provide an input or an expanded cell dependency and
/// therefore cannot be removed while admitting the entry. `cell_ref` parents
/// only need to precede the entry because they read a cell it consumes; those
/// are the sole roots the ancestor-limit escape hatch may displace. `all` is
/// the union committed to the accepted dependency graph.
struct TxParents {
    all: HashSet<ProposalShortId>,
    required: HashSet<ProposalShortId>,
    cell_ref: HashSet<ProposalShortId>,
}

/// Aggregated statistics tracked by [`PoolMap`].
#[derive(Default)]
pub struct PoolStats {
    // sum of all tx_pool tx's virtual sizes.
    pub total_tx_size: usize,
    /// Conservative resident bytes held by accepted entries.
    pub(crate) total_tx_resident_size: usize,
    // sum of all tx_pool tx's cycles.
    pub total_tx_cycles: Cycle,
    pub pending_count: usize,
    pub gap_count: usize,
    pub proposed_count: usize,
}

impl PoolStats {
    pub fn clear(&mut self) {
        self.total_tx_size = 0;
        self.total_tx_resident_size = 0;
        self.total_tx_cycles = 0;
        self.pending_count = 0;
        self.gap_count = 0;
        self.proposed_count = 0;
    }

    /// Apply one status transition without hiding an impossible underflow or
    /// overflow. The caller can then rebuild these cached counts from the
    /// authoritative entry set on the cold invariant-recovery path.
    fn checked_adjust_status_count(&mut self, remove: Option<Status>, add: Option<Status>) -> bool {
        let mut pending = self.pending_count;
        let mut gap = self.gap_count;
        let mut proposed = self.proposed_count;
        if let Some(status) = remove {
            let count = match status {
                Status::Pending => &mut pending,
                Status::Gap => &mut gap,
                Status::Proposed => &mut proposed,
            };
            let Some(next) = count.checked_sub(1) else {
                return false;
            };
            *count = next;
        }
        if let Some(status) = add {
            let count = match status {
                Status::Pending => &mut pending,
                Status::Gap => &mut gap,
                Status::Proposed => &mut proposed,
            };
            let Some(next) = count.checked_add(1) else {
                return false;
            };
            *count = next;
        }
        self.pending_count = pending;
        self.gap_count = gap;
        self.proposed_count = proposed;
        true
    }

    /// Atomically adjust cached total size and cycles for a removed tx.
    /// If either counter underflows, use `recompute` (precomputed by the caller)
    /// to recover both counters together and stay consistent.
    fn adjust_totals(
        &mut self,
        tx_size: usize,
        resident_size: usize,
        cycles: Cycle,
        recompute: Option<(usize, usize, Cycle)>,
        action: &'static str,
    ) {
        match (
            self.total_tx_size.checked_sub(tx_size),
            self.total_tx_resident_size.checked_sub(resident_size),
            self.total_tx_cycles.checked_sub(cycles),
        ) {
            (Some(size), Some(remaining_resident), Some(remaining_cycles)) => {
                self.total_tx_size = size;
                self.total_tx_resident_size = remaining_resident;
                self.total_tx_cycles = remaining_cycles;
            }
            _ => match recompute {
                Some((recomputed_size, recomputed_resident, recomputed_cycles)) => {
                    error!(
                        "tx-pool total stats underflowed when removing size {} resident {} cycles {} in {}, recomputed size {} resident {} cycles {}",
                        tx_size,
                        resident_size,
                        cycles,
                        action,
                        recomputed_size,
                        recomputed_resident,
                        recomputed_cycles
                    );
                    self.total_tx_size = recomputed_size;
                    self.total_tx_resident_size = recomputed_resident;
                    self.total_tx_cycles = recomputed_cycles;
                }
                None => panic!(
                    "tx-pool authoritative totals overflowed while repairing an underflow in {action}"
                ),
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
}

impl PoolMap {
    pub fn new(max_ancestors_count: usize) -> Self {
        PoolMap {
            entries: MultiIndexPoolEntryMap::default(),
            out_point_index: OutPointIndex::default(),
            links: TxLinksMap::new(),
            max_ancestors_count,
            stats: PoolStats::default(),
        }
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

    /// Resolve a full transaction hash through the compact proposal index
    /// without trusting the 10-byte key as identity. Any caller that starts
    /// from an outpoint or RPC hash must use this boundary instead of
    /// `get_by_id`.
    pub(crate) fn get_by_hash(&self, hash: &Byte32) -> Option<&PoolEntry> {
        let id = ProposalShortId::from_tx_hash(hash);
        self.get_by_id(&id)
            .filter(|entry| entry.inner.transaction().hash() == *hash)
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
        self.get_by_hash(&out_point.tx_hash())
            .map(|entry| &entry.inner)
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
    /// Returns `Reject` only after restoring every tentative escape-hatch
    /// eviction. On success, `inserted` distinguishes a duplicate short-id
    /// slot and `evicted` is the exact status-bearing removal cohort.
    pub(crate) fn add_entry(
        &mut self,
        mut entry: TxEntry,
        status: Status,
    ) -> Result<PoolMapAddOutcome, Reject> {
        let tx_short_id = entry.proposal_short_id();
        if self.entries.get_by_id(&tx_short_id).is_some() {
            return Ok(PoolMapAddOutcome::default());
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
        // Any failure happens before the mutating steps, so the link graph is
        // only written at step 5 and a rejected entry leaves no ghost nodes.
        // Escape-hatch removals are held in this call's local undo cohort and
        // restored before an error crosses the PoolMap authority boundary.
        self.pre_validate_entry_inputs(&entry)?;
        self.update_stat_for_add_tx(entry.size, entry.resident_size(), entry.cycles)?;

        let mut evicted = Vec::new();
        let parents = match self.check_and_prepare_ancestors(&mut entry, &mut evicted) {
            Ok(parents) => parents,
            Err(reject) => {
                self.restore_failed_add_evictions(evicted);
                return Err(reject);
            }
        };
        if let Err(reject) = self.pre_validate_entry_deps(&entry) {
            self.restore_failed_add_evictions(evicted);
            return Err(reject);
        }
        self.commit_ancestor_links(tx_short_id, parents);

        self.record_entry_edges(&entry);
        // Link children that entered the pool *before* this entry and fold
        // their weight into the entry's own descendant statistics — before
        // `insert_entry` freezes the derived keys, so the evict key already
        // reflects the children.
        let linked_descendants = self.link_and_fold_children(&mut entry);
        self.insert_entry(&entry, status);
        // Update the derived keys on both sides: the children's ancestor
        // side and the ancestors' descendant side. The entry's own
        // ancestor/descendant weights are already folded into `entry`.
        self.update_descendants_index_key_for(&entry, EntryOp::Add, &linked_descendants);
        self.update_ancestors_index_key(&entry, EntryOp::Add);
        self.track_entry_statistics(None, Some(status));
        // Apply the stat deltas *after* eviction: applying values computed
        // before it would clobber the decrements `update_stat_for_remove_tx`
        // made for the evicted entries. Overflow was prevalidated before the
        // only intervening operation (removal), so exact addition is now an
        // invariant rather than a saturating fallback that hides corruption.
        self.stats.total_tx_size = self
            .stats
            .total_tx_size
            .checked_add(entry.size)
            .expect("prevalidated tx-pool serialized total cannot overflow");
        self.stats.total_tx_resident_size = self
            .stats
            .total_tx_resident_size
            .checked_add(entry.resident_size())
            .expect("prevalidated tx-pool resident total cannot overflow");
        self.stats.total_tx_cycles = self
            .stats
            .total_tx_cycles
            .checked_add(entry.cycles)
            .expect("prevalidated tx-pool cycle total cannot overflow");
        Ok(PoolMapAddOutcome {
            inserted: true,
            evicted,
        })
    }

    fn restore_failed_add_evictions(&mut self, evicted: Vec<RemovedPoolEntry>) {
        if evicted.is_empty() {
            return;
        }
        self.restore_removed_entries_exact(evicted)
            .expect("failed PoolMap insertion must restore its local eviction cohort");
    }

    /// Restore a journal captured from a previously valid PoolMap state.
    /// Parent entries have strictly smaller recorded ancestor counts than
    /// their descendants, so this order reconstructs the original graph while
    /// every derived weight is recomputed exactly once. Any eviction or
    /// duplicate during restoration proves the caller is not rolling back the
    /// state it captured and must be treated as an authoritative failure.
    pub(crate) fn restore_removed_entries_exact(
        &mut self,
        mut removed: Vec<RemovedPoolEntry>,
    ) -> Result<Vec<(TransactionView, Status)>, Reject> {
        removed.sort_unstable_by_key(|removed| removed.entry.ancestors_count);
        let mut restored = Vec::with_capacity(removed.len());
        for mut removed in removed {
            let tx = removed.entry.transaction().clone();
            let tx_hash = tx.hash();
            let status = removed.status;
            removed.entry.reset_statistic_state();
            let result = self.add_entry(removed.entry, status);
            match result {
                Ok(PoolMapAddOutcome {
                    inserted: true,
                    evicted,
                }) if evicted.is_empty() => {
                    restored.push((tx, status));
                }
                Ok(outcome) => {
                    return Err(Reject::Malformed(
                        "pool".to_string(),
                        format!(
                            "failed exact PoolMap restore for {tx_hash}: inserted={}, evicted={}",
                            outcome.inserted,
                            outcome.evicted.len()
                        ),
                    ));
                }
                Err(reject) => {
                    return Err(Reject::Malformed(
                        "pool".to_string(),
                        format!("failed exact PoolMap restore for {tx_hash}: {reject}"),
                    ));
                }
            }
        }
        Ok(restored)
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
                return Err(Reject::Resolve(OutPointError::Dead(
                    crate::util::compact_packed(&i),
                )));
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
                return Err(Reject::Resolve(OutPointError::Dead(
                    crate::util::compact_packed(d),
                )));
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

    pub(crate) fn remove_entry_with_status(
        &mut self,
        id: &ProposalShortId,
    ) -> Option<RemovedPoolEntry> {
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
            self.update_stat_for_remove_tx(
                entry.inner.size,
                entry.inner.resident_size(),
                entry.inner.cycles,
            );
            RemovedPoolEntry {
                entry: entry.inner,
                status: entry.status,
            }
        })
    }

    pub(crate) fn remove_entry(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        self.remove_entry_with_status(id)
            .map(|removed| removed.entry)
    }

    pub(crate) fn remove_entry_and_descendants_with_status(
        &mut self,
        id: &ProposalShortId,
    ) -> Vec<RemovedPoolEntry> {
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
            .filter_map(|id| self.remove_entry_with_status(id))
            .collect()
    }

    pub(crate) fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        self.remove_entry_and_descendants_with_status(id)
            .into_iter()
            .map(|removed| removed.entry)
            .collect()
    }

    /// Compute the conflict closure of `roots` in a single multi-source
    /// traversal that both collects the set and emits it post-ordered
    /// (children before parents, as `remove_entry` requires for index
    /// weights to be subtracted against still-valid links).
    ///
    /// The traversal aborts as soon as the union exceeds `limit` unique
    /// entries, so the caller's cap (RBF rule #5's
    /// `MAX_POOL_MUTATION_CANDIDATES`) is the hard bound on cost regardless
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
                .map(|_| crate::util::compact_packed(&out_point))
        })
    }

    pub(crate) fn resolve_conflict(&mut self, tx: &TransactionView) -> Vec<ConflictEntry> {
        let mut conflicts = Vec::new();
        let mut roots = std::collections::HashMap::<ProposalShortId, OutPoint>::new();
        for i in tx.input_pts_iter() {
            if let Some(id) = self.out_point_index.get_input_ref(&i) {
                roots
                    .entry(id.clone())
                    .or_insert_with(|| crate::util::compact_packed(&i));
            }
            if let Some(ids) = self.out_point_index.get_deps_ref(&i) {
                for id in ids {
                    roots
                        .entry(id.clone())
                        .or_insert_with(|| crate::util::compact_packed(&i));
                }
            }
        }

        for (id, out_point) in roots {
            let entries = self.remove_entry_and_descendants(&id);
            if !entries.is_empty() {
                let reject = Reject::Resolve(OutPointError::Dead(out_point));
                let rejects = std::iter::repeat_n(reject, entries.len());
                conflicts.extend(entries.into_iter().zip(rejects));
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
                let removed = self
                    .links
                    .remove_child(&parent, id)
                    .expect("every parent relationship has a live parent node");
                assert!(removed, "parent relationship must be symmetric");
            }
        }
        if let Some(children) = self.links.get_children(id).cloned() {
            for child in children {
                let removed = self
                    .links
                    .remove_parent(&child, id)
                    .expect("every child relationship has a live child node");
                assert!(removed, "child relationship must be symmetric");
            }
        }
        self.links
            .remove(id)
            .expect("every accepted entry has one links node");
    }

    fn update_ancestors_index_key(&mut self, child: &TxEntry, op: EntryOp) {
        let ancestors: HashSet<ProposalShortId> =
            self.links.calc_ancestors(&child.proposal_short_id());
        for anc_id in &ancestors {
            // update parent score
            self.entries
                .modify_by_id(anc_id, |e| {
                    match op {
                        EntryOp::Remove => e.inner.sub_descendant_weight(child),
                        EntryOp::Add => e.inner.add_descendant_weight(child),
                    };
                    e.evict_key = e.inner.as_evict_key();
                })
                .expect("every ancestor link resolves to an accepted entry");
        }
    }

    fn update_descendants_index_key(&mut self, parent: &TxEntry, op: EntryOp) {
        let descendants: HashSet<ProposalShortId> =
            self.links.calc_descendants(&parent.proposal_short_id());
        self.update_descendants_index_key_for(parent, op, &descendants);
    }

    fn update_descendants_index_key_for(
        &mut self,
        parent: &TxEntry,
        op: EntryOp,
        descendants: &HashSet<ProposalShortId>,
    ) {
        for desc_id in descendants {
            // update child score
            self.entries
                .modify_by_id(desc_id, |e| {
                    match op {
                        EntryOp::Remove => e.inner.sub_ancestor_weight(parent),
                        EntryOp::Add => e.inner.add_ancestor_weight(parent),
                    };
                    e.score = e.inner.as_score_key();
                })
                .expect("every descendant link resolves to an accepted entry");
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
    /// spend or cell-dep on its outputs), fold the complete descendant
    /// closure into the entry's own statistics, and return that same closure
    /// for the caller's ancestor-key update.
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
    fn link_and_fold_children(&mut self, entry: &mut TxEntry) -> HashSet<ProposalShortId> {
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
            return HashSet::new();
        }

        for child in &children {
            let inserted = self
                .links
                .add_parent(child, tx_short_id.clone())
                .expect("every indexed child has a links node");
            assert!(inserted, "new parent relationship cannot already exist");
        }
        self.links
            .get_mut(&tx_short_id)
            .expect("new entry links node was committed before child folding")
            .children
            .extend(children);

        // Direct children can already have descendants of their own. Using
        // only the direct set undercharges a grandparent added after a whole
        // child chain and makes later subtraction corrupt its eviction key.
        // The caller already has to visit this closure to add the new parent
        // to every descendant's ancestor aggregate, so return and reuse it
        // rather than maintaining two independently derived sets.
        let descendants = self.links.calc_descendants(&tx_short_id);
        let mut delta = crate::component::entry::WeightDelta::default();
        for descendant in &descendants {
            let descendant_entry = self
                .get_by_id(descendant)
                .expect("descendant link must resolve to an accepted entry");
            delta.add_entry(&descendant_entry.inner);
        }
        entry.add_descendants_weight(delta);
        descendants
    }

    // Derive every accepted parent from the verified entry. In particular,
    // expanded dep-group members must use the same causal graph as the
    // reverse outpoint index; deriving this from raw `TransactionView`
    // cell-deps alone strands consumers when an expanded member disappears.
    fn get_tx_parents(&self, entry: &TxEntry, parent_limit: usize) -> Option<TxParents> {
        let tx = entry.transaction();
        let mut all = HashSet::with_capacity(tx.inputs().len() + tx.cell_deps().len());
        let mut required = HashSet::new();
        let mut cell_ref = HashSet::new();

        for input in tx.inputs() {
            let input_pt = input.previous_output();
            if let Some(deps) = self.out_point_index.deps.get(&input_pt) {
                for dep in deps {
                    cell_ref.insert(dep.clone());
                    all.insert(dep.clone());
                    if all.len() > parent_limit {
                        return None;
                    }
                }
            }

            let parent_hash = input_pt.tx_hash();
            let id = ProposalShortId::from_tx_hash(&parent_hash);
            if self.get_by_hash(&parent_hash).is_some() {
                required.insert(id.clone());
                all.insert(id);
                if all.len() > parent_limit {
                    return None;
                }
            }
        }
        for dep_pt in entry.related_dep_out_points() {
            let parent_hash = dep_pt.tx_hash();
            let id = ProposalShortId::from_tx_hash(&parent_hash);
            if self.get_by_hash(&parent_hash).is_some() {
                required.insert(id.clone());
                all.insert(id);
                if all.len() > parent_limit {
                    return None;
                }
            }
        }

        Some(TxParents {
            all,
            required,
            cell_ref,
        })
    }

    fn cell_ref_eviction_limit_reject() -> Reject {
        Reject::Full(format!(
            "cell-ref eviction exceeds the per-transition limit of {MAX_POOL_MUTATION_CANDIDATES}"
        ))
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
            let inserted = self
                .links
                .add_child(parent, short_id.clone())
                .expect("every planned parent has a links node");
            assert!(inserted, "new child relationship cannot already exist");
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
    /// This is an optimistic lower-bound proof, not a duplicate of the exact
    /// escape-hatch planner. Failure is returned only when removing every
    /// individually bounded cell-ref closure that preserves required output
    /// producers still cannot bring the ancestor count under the limit.
    /// Combined-cost and ordering-sensitive cases fall through to the exact
    /// planner, so pre-validation cannot reject a committable entry.
    pub(crate) fn validate_ancestor_capacity(
        &self,
        entry: &TxEntry,
        excluded: &HashSet<ProposalShortId>,
    ) -> Result<(), Reject> {
        let parent_limit = self
            .max_ancestors_count
            .saturating_sub(1)
            .saturating_add(MAX_POOL_MUTATION_CANDIDATES)
            .saturating_add(excluded.len());
        let Some(TxParents {
            all: parents,
            required,
            cell_ref: cell_ref_parents,
        }) = self.get_tx_parents(entry, parent_limit)
        else {
            return Err(Self::cell_ref_eviction_limit_reject());
        };
        let ancestors = self
            .links
            .calc_relation_ids(parents.clone(), Relation::Parents);
        let effective_parent_count = parents.difference(excluded).count();
        if effective_parent_count.saturating_sub(self.max_ancestors_count.saturating_sub(1))
            > MAX_POOL_MUTATION_CANDIDATES
        {
            return Err(Self::cell_ref_eviction_limit_reject());
        }
        let effective_ancestors = ancestors
            .difference(excluded)
            .cloned()
            .collect::<HashSet<_>>();
        if effective_ancestors.len() < self.max_ancestors_count {
            return Ok(());
        }

        // Prove impossibility without mutating. Assume every individually
        // bounded cell-ref closure that preserves all required producers can
        // be removed, even if their combined physical cost would later exceed
        // the escape-hatch cap. If the entry still cannot fit under that
        // optimistic lower bound, the authoritative attempt is certain to
        // fail and removing/restoring the RBF cohort would be pure churn.
        // Borderline/cap-sensitive cases fall through to the exact bounded
        // planner after the RBF cohort has been removed.
        let effective_required = required
            .difference(excluded)
            .cloned()
            .collect::<HashSet<_>>();
        let mut potentially_removed = HashSet::new();
        for root in cell_ref_parents.difference(excluded) {
            let roots = HashSet::from([root.clone()]);
            let ConflictClosure::Complete { removal_set, .. } =
                self.conflict_closure(&roots, MAX_POOL_MUTATION_CANDIDATES)
            else {
                continue;
            };
            if removal_set.is_disjoint(&effective_required) {
                potentially_removed.extend(removal_set);
            }
        }
        let remaining_parents = parents
            .difference(excluded)
            .filter(|id| !potentially_removed.contains(*id))
            .cloned()
            .collect::<HashSet<_>>();
        let remaining_ancestors = self
            .links
            .calc_relation_ids(remaining_parents, Relation::Parents)
            .difference(excluded)
            .filter(|id| !potentially_removed.contains(*id))
            .count();
        if remaining_ancestors >= self.max_ancestors_count {
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
        evicted: &mut Vec<RemovedPoolEntry>,
    ) -> Result<HashSet<ProposalShortId>, Reject> {
        let surviving_parent_limit = self.max_ancestors_count.saturating_sub(1);
        let parent_limit = surviving_parent_limit.saturating_add(MAX_POOL_MUTATION_CANDIDATES);
        let Some(TxParents {
            all: parents,
            required,
            cell_ref: cell_ref_parents,
        }) = self.get_tx_parents(entry, parent_limit)
        else {
            return Err(Self::cell_ref_eviction_limit_reject());
        };

        // Reject an obviously over-bound fan-out before traversing the
        // complete ancestor graph. At most `max_ancestors_count - 1` direct
        // parents can survive beside the new entry, and every additional
        // direct parent is itself one physical removal even when one selected
        // root cascades through several of them.
        if parents.len().saturating_sub(surviving_parent_limit) > MAX_POOL_MUTATION_CANDIDATES {
            return Err(Self::cell_ref_eviction_limit_reject());
        }

        let ancestors = self
            .links
            .calc_relation_ids(parents.clone(), Relation::Parents);

        let ancestors_count = ancestors
            .len()
            .checked_add(1)
            .expect("accepted ancestor count cannot overflow");
        if ancestors_count <= self.max_ancestors_count {
            self.apply_ancestor_weights(entry, &ancestors)?;
            return Ok(parents);
        }

        // Plan against immutable indexes first. The old loop removed entries
        // while still discovering how large the cascade was, allowing one
        // shared dep cell to turn a remote admission into a pool-wide
        // write-lock mutation. Sorting only the actual cell-ref parents also
        // avoids scanning the complete eviction index.
        let mut candidates = cell_ref_parents
            .iter()
            .filter_map(|id| {
                self.get_by_id(id)
                    .map(|pool_entry| (pool_entry.evict_key.clone(), id.clone()))
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable_by(|left, right| {
            left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1))
        });

        let mut roots = HashSet::new();
        let mut plan = None;
        for (_, candidate) in candidates {
            let mut proposed_roots = roots.clone();
            proposed_roots.insert(candidate);
            let (removal, removal_set) =
                match self.conflict_closure(&proposed_roots, MAX_POOL_MUTATION_CANDIDATES) {
                    ConflictClosure::Complete {
                        removal,
                        removal_set,
                    } => (removal, removal_set),
                    ConflictClosure::Exceeded { .. } => {
                        return Err(Self::cell_ref_eviction_limit_reject());
                    }
                };
            // The resolved entry is valid only while every producer of an
            // input or expanded dep remains accepted. An ordering-only
            // cell-ref root whose cascade reaches such a producer is not an
            // eviction candidate at all.
            if !removal_set.is_disjoint(&required) {
                continue;
            }
            roots = proposed_roots;
            let remaining_parents = parents
                .difference(&removal_set)
                .cloned()
                .collect::<HashSet<_>>();
            let remaining_ancestors = self
                .links
                .calc_relation_ids(remaining_parents.clone(), Relation::Parents);
            if remaining_ancestors.len() < self.max_ancestors_count {
                plan = Some((removal, removal_set, remaining_parents, remaining_ancestors));
                break;
            }
        }

        let Some((removal, removal_set, parents, ancestors)) = plan else {
            return Err(Reject::ExceededMaximumAncestorsCount);
        };
        self.apply_ancestor_weights(entry, &ancestors)?;

        // `conflict_closure` is post-ordered, so every child is removed while
        // its ancestor links still exist. No fallible work remains after this
        // point; the caller receives the exact status-bearing undo cohort.
        for id in removal {
            if removal_set.contains(&id) {
                evicted.push(
                    self.remove_entry_with_status(&id)
                        .expect("planned cell-ref eviction entry remains present"),
                );
            }
        }
        Ok(parents)
    }

    fn remove_entry_edges(&mut self, entry: &TxEntry) {
        let id = entry.proposal_short_id();
        let inputs: HashSet<_> = entry.transaction().input_pts_iter().collect();
        for input in &inputs {
            // release input record
            let indexed = self
                .out_point_index
                .remove_input(input)
                .expect("every accepted input has one index owner");
            assert_eq!(indexed, id, "accepted input index owner must match entry");
        }
        for d in entry.related_dep_out_points().cloned() {
            if !inputs.contains(&d) {
                assert!(
                    self.out_point_index.delete_txid_by_dep(d, &id),
                    "every accepted dep has a reverse index owner"
                );
            }
        }

        let expected_headers = entry
            .transaction()
            .header_deps()
            .into_iter()
            .collect::<Vec<_>>();
        let indexed_headers = self.out_point_index.header_deps.remove(&id);
        if expected_headers.is_empty() {
            assert!(indexed_headers.is_none());
        } else {
            assert_eq!(
                indexed_headers.as_deref(),
                Some(expected_headers.as_slice())
            );
        }
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
        if !self.stats.checked_adjust_status_count(remove, add) {
            let (pending, gap, proposed) = self.recompute_status_counts();
            error!(
                "tx-pool status counters drifted during transition {:?} -> {:?}; recomputed pending {} gap {} proposed {}",
                remove, add, pending, gap, proposed
            );
            self.stats.pending_count = pending;
            self.stats.gap_count = gap;
            self.stats.proposed_count = proposed;
        }
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

    fn recompute_status_counts(&self) -> (usize, usize, usize) {
        self.entries.iter().fold(
            (0usize, 0usize, 0usize),
            |(pending, gap, proposed), (_, entry)| match entry.status {
                Status::Pending => (pending + 1, gap, proposed),
                Status::Gap => (pending, gap + 1, proposed),
                Status::Proposed => (pending, gap, proposed + 1),
            },
        )
    }

    fn recompute_total_stat(&self) -> Option<(usize, usize, Cycle)> {
        self.entries.iter().try_fold(
            (0usize, 0usize, 0 as Cycle),
            |(total_size, total_resident_size, total_cycles), (_, entry)| {
                Some((
                    total_size.checked_add(entry.inner.size)?,
                    total_resident_size.checked_add(entry.inner.resident_size())?,
                    total_cycles.checked_add(entry.inner.cycles)?,
                ))
            },
        )
    }

    /// Repair cached totals from the authoritative entries. This is a cold
    /// invariant-recovery path, never part of ordinary insertion/removal.
    pub(crate) fn repair_total_statistics(&mut self, action: &'static str) {
        let (size, resident, cycles) = self
            .recompute_total_stat()
            .unwrap_or_else(|| panic!("tx-pool authoritative totals overflowed in {action}"));
        error!(
            "tx-pool total counters drifted in {}; recomputed serialized {} resident {} cycles {}",
            action, size, resident, cycles
        );
        self.stats.total_tx_size = size;
        self.stats.total_tx_resident_size = resident;
        self.stats.total_tx_cycles = cycles;
    }

    /// Calculate size and cycles statistics for adding a tx.
    fn update_stat_for_add_tx(
        &self,
        tx_size: usize,
        resident_size: usize,
        cycles: Cycle,
    ) -> Result<(), Reject> {
        self.stats
            .total_tx_size
            .checked_add(tx_size)
            .ok_or_else(|| {
                Reject::Full(format!(
                    "tx-pool total_tx_size {} overflows by add {}",
                    self.stats.total_tx_size, tx_size
                ))
            })?;
        self.stats
            .total_tx_resident_size
            .checked_add(resident_size)
            .ok_or_else(|| {
                Reject::Full(format!(
                    "tx-pool total_tx_resident_size {} overflows by add {}",
                    self.stats.total_tx_resident_size, resident_size
                ))
            })?;
        self.stats
            .total_tx_cycles
            .checked_add(cycles)
            .ok_or_else(|| {
                Reject::Full(format!(
                    "tx-pool total_tx_cycles {} overflows by add {}",
                    self.stats.total_tx_cycles, cycles
                ))
            })?;
        Ok(())
    }

    /// Update size and cycles statistics for remove tx.
    /// Cycles overflow is possible because cycle counts are not always accurate.
    fn update_stat_for_remove_tx(&mut self, tx_size: usize, resident_size: usize, cycles: Cycle) {
        let needs_recompute = self.stats.total_tx_size.checked_sub(tx_size).is_none()
            || self
                .stats
                .total_tx_resident_size
                .checked_sub(resident_size)
                .is_none()
            || self.stats.total_tx_cycles.checked_sub(cycles).is_none();
        let recompute = if needs_recompute {
            self.recompute_total_stat()
        } else {
            None
        };
        self.stats
            .adjust_totals(tx_size, resident_size, cycles, recompute, "remove_tx");
    }
}
