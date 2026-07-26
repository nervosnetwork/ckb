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
use ckb_logger::{debug, error};
use ckb_types::core::error::OutPointError;
use ckb_types::core::{Cycle, FeeRate};
use ckb_types::packed::OutPoint;
use ckb_types::{
    core::TransactionView,
    packed::{Byte32, ProposalShortId},
};
use multi_index_map::MultiIndexMap;
use std::collections::{HashMap, HashSet};
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
    /// Complete raw transaction identity. All hash-originated lookups use
    /// this unique index; the proposal short ID below is a protocol slot.
    #[multi_index(hashed_unique)]
    pub(crate) hash: Byte32,
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

/// Complete accepted-entry record emitted by a successfully applied removal.
///
/// The proposal-window status travels with the entry so downstream effects
/// never have to reconstruct authoritative state from a transaction alone.
#[derive(Debug, Clone)]
pub(crate) struct RemovedPoolEntry {
    pub(crate) entry: TxEntry,
    pub(crate) status: Status,
}

/// The closed set of policy authorities allowed to remove accepted entries
/// during ordinary transaction admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemovalCause {
    Replacement,
    SizeLimit,
}

#[derive(Debug, Clone)]
pub(crate) struct PlannedRemoval {
    pub(crate) id: ProposalShortId,
    pub(crate) hash: Byte32,
    pub(crate) status: Status,
    pub(crate) cause: RemovalCause,
}

/// Immutable decision for one accepted-pool admission. Every ordinary
/// rejection is returned while constructing this value; applying it only
/// moves prevalidated values.
#[derive(Debug)]
pub(crate) struct PoolMutationPlan {
    pub(crate) candidate: TxEntry,
    pub(crate) status: Status,
    pub(crate) removals: Vec<PlannedRemoval>,
    candidate_parents: HashSet<ProposalShortId>,
    post_total_tx_size: usize,
    post_total_resident_size: usize,
    post_total_cycles: Cycle,
}

#[derive(Debug)]
pub(crate) struct AppliedRemoval {
    pub(crate) removed: RemovedPoolEntry,
    pub(crate) cause: RemovalCause,
}

#[derive(Debug)]
pub(crate) struct AppliedPoolMutation {
    pub(crate) removals: Vec<AppliedRemoval>,
}

/// The canonical out-point memberships published for one accepted entry.
///
/// Transaction and dep-group resolution may expose the same out-point more
/// than once. The reverse indexes are sets, so validation, publication and
/// removal must all operate on this same normalized keyset. Replaying the raw
/// iterator during removal would otherwise try to delete one logical
/// membership twice and turn a valid transaction into an authoritative
/// invariant failure.
struct EntryOutPointEdges {
    inputs: HashSet<OutPoint>,
    deps: HashSet<OutPoint>,
}

impl EntryOutPointEdges {
    fn from_entry(entry: &TxEntry) -> Self {
        let inputs = entry
            .transaction()
            .input_pts_iter()
            .map(|out_point| crate::util::compact_packed(&out_point))
            .collect::<HashSet<_>>();
        let deps = entry
            .related_dep_out_points()
            .filter(|out_point| !inputs.contains(*out_point))
            .map(crate::util::compact_packed)
            .collect::<HashSet<_>>();
        Self { inputs, deps }
    }
}

/// Aggregated statistics tracked by [`PoolMap`].
#[derive(Default)]
#[cfg_attr(test, derive(Clone))]
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

    pub(crate) fn get_by_hash(&self, hash: &Byte32) -> Option<&PoolEntry> {
        self.entries.get_by_hash(hash)
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
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

    /// Build the one immutable admission decision against this stable pool
    /// generation. `mandatory_removal` is the already validated, post-ordered
    /// RBF closure. Capacity eviction is simulated with a sparse overlay over
    /// the existing ordered index; no accepted membership changes here.
    pub(crate) fn plan_mutation(
        &self,
        mut candidate: TxEntry,
        status: Status,
        mandatory_removal: &[ProposalShortId],
        max_tx_pool_size: usize,
        max_resident_size: usize,
    ) -> Result<PoolMutationPlan, Reject> {
        let candidate_id = candidate.proposal_short_id();
        let candidate_hash = candidate.transaction().hash();
        if self.get_by_hash(&candidate_hash).is_some() {
            return Err(Reject::Duplicated(candidate_hash));
        }
        if self.get_by_id(&candidate_id).is_some() {
            return Err(Reject::Full(format!(
                "proposal short-id collision while planning {candidate_hash}"
            )));
        }

        let mut removed = HashSet::with_capacity(mandatory_removal.len());
        let mut removals = Vec::with_capacity(mandatory_removal.len());
        for id in mandatory_removal {
            if !removed.insert(id.clone()) {
                continue;
            }
            let entry = self.get_by_id(id).ok_or_else(|| {
                Reject::Malformed(
                    "pool".to_string(),
                    format!("planned RBF victim {id} is missing"),
                )
            })?;
            removals.push(PlannedRemoval {
                id: id.clone(),
                hash: entry.hash.clone(),
                status: entry.status,
                cause: RemovalCause::Replacement,
            });
        }
        if removed.len() > MAX_POOL_MUTATION_CANDIDATES {
            return Err(Reject::Full(format!(
                "pool mutation exceeds the per-transition limit of {MAX_POOL_MUTATION_CANDIDATES}"
            )));
        }

        self.pre_validate_entry_inputs_excluding(&candidate, &removed)?;
        if self.has_surviving_causal_child(&candidate, &removed) {
            return Err(Reject::Malformed(
                "pool".to_string(),
                "ordinary admission would introduce a late causal parent".to_string(),
            ));
        }
        candidate.reset_statistic_state();
        let (candidate_parents, candidate_ancestors) =
            self.prepare_entry_for_plan(&mut candidate, &removed)?;

        let mut total_size = self.stats.total_tx_size;
        let mut total_resident = self.stats.total_tx_resident_size;
        let mut total_cycles = self.stats.total_tx_cycles;
        for id in &removed {
            let entry = &self
                .get_by_id(id)
                .expect("planned mandatory victim remains present")
                .inner;
            total_size = total_size.checked_sub(entry.size).ok_or_else(|| {
                Reject::Malformed("pool".to_string(), "serialized total underflow".to_string())
            })?;
            total_resident = total_resident
                .checked_sub(entry.resident_size())
                .ok_or_else(|| {
                    Reject::Malformed("pool".to_string(), "resident total underflow".to_string())
                })?;
            total_cycles = total_cycles.checked_sub(entry.cycles).ok_or_else(|| {
                Reject::Malformed("pool".to_string(), "cycle total underflow".to_string())
            })?;
        }
        total_size = total_size.checked_add(candidate.size).ok_or_else(|| {
            Reject::Malformed("pool".to_string(), "serialized total overflow".to_string())
        })?;
        total_resident = total_resident
            .checked_add(candidate.resident_size())
            .ok_or_else(|| {
                Reject::Malformed("pool".to_string(), "resident total overflow".to_string())
            })?;
        total_cycles = total_cycles.checked_add(candidate.cycles).ok_or_else(|| {
            Reject::Malformed("pool".to_string(), "cycle total overflow".to_string())
        })?;

        // Only surviving ancestors of the bounded removal union and the
        // candidate can have a different eviction key.
        let mut overlay = HashMap::<ProposalShortId, (TxEntry, Status)>::new();
        self.adjust_virtual_ancestors(&removed, &removed, &mut overlay);
        for ancestor in &candidate_ancestors {
            let item = overlay.entry(ancestor.clone()).or_insert_with(|| {
                let entry = self
                    .get_by_id(ancestor)
                    .expect("candidate ancestor remains present");
                (entry.inner.clone(), entry.status)
            });
            item.0.add_descendant_weight(&candidate);
        }
        overlay.insert(candidate_id.clone(), (candidate.clone(), status));

        while total_size > max_tx_pool_size || total_resident > max_resident_size {
            let root = self
                .next_virtual_evict(Status::Pending, &removed, &overlay)
                .or_else(|| self.next_virtual_evict(Status::Gap, &removed, &overlay))
                .or_else(|| self.next_virtual_evict(Status::Proposed, &removed, &overlay))
                .ok_or_else(|| {
                    Reject::Malformed(
                        "pool".to_string(),
                        "over-budget virtual pool has no eviction candidate".to_string(),
                    )
                })?;
            if root == candidate_id {
                return Err(Reject::Full(format!(
                    "the fee_rate for this transaction is: {}",
                    candidate.fee_rate()
                )));
            }
            let closure = self.virtual_descendant_postorder(
                &root,
                &removed,
                MAX_POOL_MUTATION_CANDIDATES.saturating_sub(removed.len()),
            )?;
            if closure
                .iter()
                .any(|id| candidate_ancestors.contains(id) || id == &candidate_id)
            {
                return Err(Reject::Full(format!(
                    "the fee_rate for this transaction is: {}",
                    candidate.fee_rate()
                )));
            }
            let next_removed = removed
                .iter()
                .cloned()
                .chain(closure.iter().cloned())
                .collect::<HashSet<_>>();
            let closure_set = closure.iter().cloned().collect::<HashSet<_>>();
            self.adjust_virtual_ancestors(&closure_set, &next_removed, &mut overlay);
            for id in closure {
                if !removed.insert(id.clone()) {
                    continue;
                }
                let (entry, entry_status, hash) =
                    if let Some((entry, entry_status)) = overlay.get(&id) {
                        (entry, *entry_status, entry.transaction().hash())
                    } else {
                        let entry = self
                            .get_by_id(&id)
                            .expect("virtual eviction candidate remains present");
                        (&entry.inner, entry.status, entry.hash.clone())
                    };
                total_size = total_size
                    .checked_sub(entry.size)
                    .expect("virtual serialized removal was pre-accounted");
                total_resident = total_resident
                    .checked_sub(entry.resident_size())
                    .expect("virtual resident removal was pre-accounted");
                total_cycles = total_cycles
                    .checked_sub(entry.cycles)
                    .expect("virtual cycle removal was pre-accounted");
                removals.push(PlannedRemoval {
                    id,
                    hash,
                    status: entry_status,
                    cause: RemovalCause::SizeLimit,
                });
            }
        }

        Ok(PoolMutationPlan {
            candidate,
            status,
            removals,
            candidate_parents,
            post_total_tx_size: total_size,
            post_total_resident_size: total_resident,
            post_total_cycles: total_cycles,
        })
    }

    fn pre_validate_entry_inputs_excluding(
        &self,
        entry: &TxEntry,
        excluded: &HashSet<ProposalShortId>,
    ) -> Result<(), Reject> {
        for input in entry.transaction().input_pts_iter() {
            if let Some(owner) = self.out_point_index.get_input_ref(&input)
                && !excluded.contains(owner)
            {
                return Err(Reject::Resolve(OutPointError::Dead(
                    crate::util::compact_packed(&input),
                )));
            }
        }
        Ok(())
    }

    fn has_surviving_causal_child(
        &self,
        entry: &TxEntry,
        excluded: &HashSet<ProposalShortId>,
    ) -> bool {
        entry.transaction().output_pts().into_iter().any(|output| {
            self.out_point_index
                .get_input_ref(&output)
                .is_some_and(|id| !excluded.contains(id))
                || self
                    .out_point_index
                    .get_deps_ref(&output)
                    .is_some_and(|ids| ids.iter().any(|id| !excluded.contains(id)))
        })
    }

    fn prepare_entry_for_plan(
        &self,
        entry: &mut TxEntry,
        excluded: &HashSet<ProposalShortId>,
    ) -> Result<(HashSet<ProposalShortId>, HashSet<ProposalShortId>), Reject> {
        let parents = self
            .get_tx_parents(
                entry,
                self.max_ancestors_count.saturating_add(excluded.len()),
            )
            .ok_or(Reject::ExceededMaximumAncestorsCount)?
            .difference(excluded)
            .cloned()
            .collect::<HashSet<_>>();
        let ancestors = self
            .links
            .calc_relation_ids(parents.clone(), Relation::Parents)
            .difference(excluded)
            .cloned()
            .collect::<HashSet<_>>();
        if ancestors.len() >= self.max_ancestors_count {
            return Err(Reject::ExceededMaximumAncestorsCount);
        }
        self.apply_ancestor_weights(entry, &ancestors)?;
        Ok((parents, ancestors))
    }

    fn next_virtual_evict(
        &self,
        status: Status,
        removed: &HashSet<ProposalShortId>,
        overlay: &HashMap<ProposalShortId, (TxEntry, Status)>,
    ) -> Option<ProposalShortId> {
        let base = self
            .entries
            .iter_by_evict_key()
            .find(|entry| {
                entry.status == status
                    && !removed.contains(&entry.id)
                    && !overlay.contains_key(&entry.id)
            })
            .map(|entry| (entry.evict_key.clone(), entry.id.clone()));
        let sparse = overlay
            .iter()
            .filter(|(id, (_, entry_status))| *entry_status == status && !removed.contains(*id))
            .map(|(id, (entry, _))| (entry.as_evict_key(), id.clone()))
            .min();
        match (base, sparse) {
            (Some(base), Some(sparse)) => Some(if sparse < base { sparse.1 } else { base.1 }),
            (Some(base), None) => Some(base.1),
            (None, Some(sparse)) => Some(sparse.1),
            (None, None) => None,
        }
    }

    fn virtual_descendant_postorder(
        &self,
        root: &ProposalShortId,
        removed: &HashSet<ProposalShortId>,
        limit: usize,
    ) -> Result<Vec<ProposalShortId>, Reject> {
        let mut visited = HashSet::new();
        let mut ordered = Vec::new();
        let mut stack = vec![(root.clone(), false)];
        while let Some((id, processed)) = stack.pop() {
            if removed.contains(&id) {
                continue;
            }
            if processed {
                ordered.push(id);
                continue;
            }
            if !visited.insert(id.clone()) {
                continue;
            }
            if self.get_by_id(&id).is_some() && visited.len() > limit {
                return Err(Reject::Full(format!(
                    "pool mutation exceeds the per-transition limit of {MAX_POOL_MUTATION_CANDIDATES}"
                )));
            }
            stack.push((id.clone(), true));
            if let Some(children) = self.links.get_children(&id) {
                for child in children {
                    if !removed.contains(child) {
                        stack.push((child.clone(), false));
                    }
                }
            }
        }
        Ok(ordered)
    }

    fn adjust_virtual_ancestors(
        &self,
        removed_now: &HashSet<ProposalShortId>,
        removed_after: &HashSet<ProposalShortId>,
        overlay: &mut HashMap<ProposalShortId, (TxEntry, Status)>,
    ) {
        for id in removed_now {
            let removed_entry = overlay
                .get(id)
                .map(|(entry, _)| entry.clone())
                .or_else(|| self.get_by_id(id).map(|entry| entry.inner.clone()))
                .expect("virtual removal resolves to an accepted entry");
            for ancestor in self.links.calc_ancestors(id) {
                if removed_after.contains(&ancestor) {
                    continue;
                }
                let item = overlay.entry(ancestor.clone()).or_insert_with(|| {
                    let entry = self
                        .get_by_id(&ancestor)
                        .expect("surviving virtual ancestor remains accepted");
                    (entry.inner.clone(), entry.status)
                });
                item.0.sub_descendant_weight(&removed_entry);
            }
        }
    }

    /// Apply a previously constructed plan. All transaction-shaped errors,
    /// arithmetic and identity checks have already completed; any mismatch is
    /// a programming defect rather than an ordinary rejection path.
    pub(crate) fn apply_mutation(&mut self, plan: PoolMutationPlan) -> AppliedPoolMutation {
        let PoolMutationPlan {
            candidate,
            status,
            removals,
            candidate_parents,
            post_total_tx_size,
            post_total_resident_size,
            post_total_cycles,
        } = plan;
        let mut applied = Vec::with_capacity(removals.len());
        for planned in removals {
            let current = self
                .get_by_id(&planned.id)
                .expect("planned removal remains present until total apply");
            assert_eq!(current.hash, planned.hash);
            assert_eq!(current.status, planned.status);
            let removed = self
                .remove_entry_with_status(&planned.id)
                .expect("validated removal is infallible");
            applied.push(AppliedRemoval {
                removed,
                cause: planned.cause,
            });
        }

        let candidate_id = candidate.proposal_short_id();
        let candidate_hash = candidate.transaction().hash();
        assert!(self.get_by_hash(&candidate_hash).is_none());
        assert!(self.get_by_id(&candidate_id).is_none());
        let edges = EntryOutPointEdges::from_entry(&candidate);
        debug_assert!(self.pre_validate_entry_inputs(&edges).is_ok());
        self.commit_ancestor_links(candidate_id, candidate_parents);
        self.record_entry_edges(&candidate, &edges);
        self.insert_entry(&candidate, status);
        self.update_ancestors_index_key(&candidate, EntryOp::Add);
        self.track_entry_statistics(None, Some(status));
        self.stats.total_tx_size = self
            .stats
            .total_tx_size
            .checked_add(candidate.size)
            .expect("planned serialized total cannot overflow");
        self.stats.total_tx_resident_size = self
            .stats
            .total_tx_resident_size
            .checked_add(candidate.resident_size())
            .expect("planned resident total cannot overflow");
        self.stats.total_tx_cycles = self
            .stats
            .total_tx_cycles
            .checked_add(candidate.cycles)
            .expect("planned cycle total cannot overflow");
        assert_eq!(self.stats.total_tx_size, post_total_tx_size);
        assert_eq!(self.stats.total_tx_resident_size, post_total_resident_size);
        assert_eq!(self.stats.total_tx_cycles, post_total_cycles);
        AppliedPoolMutation { removals: applied }
    }

    /// Internal instrumentation inserts through the same immutable Plan and
    /// total Apply as ordinary admission. The permissive child-first graph
    /// constructor is isolated in the dedicated test module.
    #[cfg(feature = "internal")]
    pub(crate) fn plug_entry(&mut self, entry: TxEntry, status: Status) -> Result<bool, Reject> {
        if self.get_by_hash(&entry.transaction().hash()).is_some() {
            return Ok(false);
        }
        let plan = self.plan_mutation(entry, status, &[], usize::MAX, usize::MAX)?;
        self.apply_mutation(plan);
        Ok(true)
    }

    /// Defensive read-only check: none of the entry's inputs is already
    /// consumed by another in-pool transaction.
    ///
    /// Every mutation caller decides input conflicts before Apply. No mutation
    /// occurs between this check and `record_entry_edges` that can occupy an
    /// input.
    fn pre_validate_entry_inputs(&self, edges: &EntryOutPointEdges) -> Result<(), Reject> {
        for input in &edges.inputs {
            if let Some(conflict) = self.out_point_index.get_input_ref(input) {
                debug!(
                    "pre_validate_entry_inputs: input {:?} already consumed by {}",
                    input, conflict
                );
                return Err(Reject::Resolve(OutPointError::Dead(input.clone())));
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

    fn record_entry_edges(&mut self, entry: &TxEntry, edges: &EntryOutPointEdges) {
        let tx_short_id: ProposalShortId = entry.proposal_short_id();
        let header_deps = entry.transaction().header_deps();

        // if input reference a in-pool output, connect it
        // otherwise, record input for conflict check
        //
        // Cannot fail: immutable Plan (or the isolated fixture constructor)
        // ran `pre_validate_entry_inputs` before this point. Dependency
        // readers intentionally coexist with a pool spender of the same
        // outpoint.
        for input in &edges.inputs {
            self.out_point_index
                .insert_input(input.clone(), tx_short_id.clone())
                .expect("entry inputs pre-validated as unoccupied");
        }

        // record dep-txid
        for dep in &edges.deps {
            self.out_point_index
                .insert_deps(dep.clone(), tx_short_id.clone());
        }
        // record header_deps
        if !header_deps.is_empty() {
            self.out_point_index
                .header_deps
                .insert(tx_short_id, header_deps.into_iter().collect());
        }
    }

    // Derive every accepted parent from the verified entry. In particular,
    // expanded dep-group members must use the same causal graph as the
    // reverse outpoint index; deriving this from raw `TransactionView`
    // cell-deps alone strands consumers when an expanded member disappears.
    fn get_tx_parents(
        &self,
        entry: &TxEntry,
        parent_limit: usize,
    ) -> Option<HashSet<ProposalShortId>> {
        let tx = entry.transaction();
        let mut parents = HashSet::with_capacity(tx.inputs().len() + tx.cell_deps().len());

        for input in tx.inputs() {
            let input_pt = input.previous_output();
            let parent_hash = input_pt.tx_hash();
            let id = ProposalShortId::from_tx_hash(&parent_hash);
            if self.get_by_hash(&parent_hash).is_some() {
                parents.insert(id);
                if parents.len() > parent_limit {
                    return None;
                }
            }
        }
        for dep_pt in entry.related_dep_out_points() {
            let parent_hash = dep_pt.tx_hash();
            let id = ProposalShortId::from_tx_hash(&parent_hash);
            if self.get_by_hash(&parent_hash).is_some() {
                parents.insert(id);
                if parents.len() > parent_limit {
                    return None;
                }
            }
        }

        Some(parents)
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
    /// Must only be called by total Apply (or the isolated fixture
    /// constructor) after every fallible validation: writing a link node
    /// earlier would leave a ghost node behind on error.
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

    fn remove_entry_edges(&mut self, entry: &TxEntry) {
        let id = entry.proposal_short_id();
        let edges = EntryOutPointEdges::from_entry(entry);
        for input in &edges.inputs {
            // release input record
            let indexed = self
                .out_point_index
                .remove_input(input)
                .expect("every accepted input has one index owner");
            assert_eq!(indexed, id, "accepted input index owner must match entry");
        }
        for dep in edges.deps {
            assert!(
                self.out_point_index.delete_txid_by_dep(dep, &id),
                "every accepted dep has a reverse index owner"
            );
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
        let tx_hash = entry.transaction().hash();
        let score = entry.as_score_key();
        let evict_key = entry.as_evict_key();
        self.entries
            .try_insert(PoolEntry {
                hash: tx_hash,
                id: tx_short_id,
                score,
                status,
                inner: entry.clone(),
                evict_key,
            })
            .expect("full hash and proposal slot were prevalidated");
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
