//! Top-level Pool type, methods, and tests
extern crate rustc_hash;
extern crate slab;
#[cfg(test)]
#[path = "tests/pool_map_audit.rs"]
mod audit;
use super::links::TxLinks;
use crate::TxEntry;
use crate::component::entry::WeightError;
use crate::component::links::{Relation, TxLinksMap};
use crate::component::out_point_index::OutPointIndex;
use crate::component::sort_key::{AncestorsScoreSortKey, EvictKey};
use crate::constants::MAX_POOL_MUTATION_CANDIDATES;
use crate::error::Reject;
use ckb_types::core::error::OutPointError;
use ckb_types::core::{Cycle, FeeRate};
use ckb_types::packed::OutPoint;
use ckb_types::prelude::Unpack;
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
    pub(crate) entry: TxEntry,
    links: TxLinks,
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
    candidate_edges: EntryOutPointEdges,
    survivor_updates: Vec<(ProposalShortId, TxEntry)>,
    post_stats: PoolStats,
}

/// A pool planning defect is distinct from transaction policy. Every variant
/// means an already-published accepted projection disagreed with its primary
/// entry map; untrusted transaction shape cannot construct these values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PoolMutationFault {
    MissingEntry(&'static str),
    ProjectionMismatch(&'static str),
    CounterOverflow(&'static str),
    CandidateIdentityDrift,
    Weight(WeightError),
}

impl From<WeightError> for PoolMutationFault {
    fn from(error: WeightError) -> Self {
        Self::Weight(error)
    }
}

/// A read-only pool decision either rejects transaction policy or proves that
/// the accepted projection is not internally representable.
#[derive(Debug)]
pub(crate) enum PoolMutationPlanningError {
    Policy(Reject),
    Fault(PoolMutationFault),
}

impl From<Reject> for PoolMutationPlanningError {
    fn from(reject: Reject) -> Self {
        Self::Policy(reject)
    }
}

impl From<PoolMutationFault> for PoolMutationPlanningError {
    fn from(fault: PoolMutationFault) -> Self {
        Self::Fault(fault)
    }
}

/// Exclusive proof that the pool generation used by Plan cannot change before
/// Apply. The decision is private and this is its only production consumer.
pub(crate) struct PreparedPoolMutation<'pool> {
    pool: &'pool mut PoolMap,
    plan: PoolMutationPlan,
}

#[derive(Debug, Clone)]
struct ProjectionRemoval {
    id: ProposalShortId,
    hash: Byte32,
    status: Status,
    entry: TxEntry,
    links: TxLinks,
}

#[derive(Debug)]
struct PoolRemovalPlan {
    removed: HashSet<ProposalShortId>,
    removals: Vec<ProjectionRemoval>,
    survivor_updates: Vec<(ProposalShortId, TxEntry)>,
    post_stats: PoolStats,
}

/// Exclusive accepted-pool removal transaction used by chain, expiry and
/// administrative paths. Planning clones only the affected causal projection;
/// Apply performs no arithmetic, lookup-dependent branching or allocation.
pub(crate) struct PreparedPoolRemoval<'pool> {
    pool: &'pool mut PoolMap,
    plan: PoolRemovalPlan,
}

#[derive(Debug)]
struct PoolStatusPlan {
    transitions: Vec<(ProposalShortId, Status)>,
    post_stats: PoolStats,
}

pub(crate) struct PreparedPoolStatus<'pool> {
    pool: &'pool mut PoolMap,
    plan: PoolStatusPlan,
}

impl PreparedPoolStatus<'_> {
    pub(crate) fn apply(self) {
        let PoolStatusPlan {
            transitions,
            post_stats,
        } = self.plan;
        for (id, target) in transitions {
            let _ = self.pool.entries.modify_by_id(&id, |entry| {
                entry.status = target;
            });
        }
        self.pool.stats = post_stats;
        self.pool.publish_stats_metrics();
    }
}

impl PreparedPoolRemoval<'_> {
    pub(crate) fn entries(&self) -> impl DoubleEndedIterator<Item = &TxEntry> + ExactSizeIterator {
        self.plan.removals.iter().map(|removal| &removal.entry)
    }

    pub(crate) fn records(
        &self,
    ) -> impl DoubleEndedIterator<Item = (Status, &TxEntry)> + ExactSizeIterator {
        self.plan
            .removals
            .iter()
            .map(|removal| (removal.status, &removal.entry))
    }

    /// Query the virtual accepted overlay proven by this removal plan. The
    /// exclusive borrow keeps the answer valid until Apply, so dependency
    /// publication needs neither a post-mutation lookup nor rollback.
    pub(crate) fn contains_output_after_apply(&self, out_point: &OutPoint) -> bool {
        let hash = out_point.tx_hash();
        let id = ProposalShortId::from_tx_hash(&hash);
        if self.plan.removed.contains(&id) {
            return false;
        }
        self.pool.get_by_hash(&hash).is_some_and(|entry| {
            let index: u32 = out_point.index().unpack();
            (index as usize) < entry.inner.transaction().outputs().len()
        })
    }

    pub(crate) fn apply(self) -> Vec<TxEntry> {
        self.pool.apply_prepared_removal(self.plan)
    }
}

impl PreparedPoolMutation<'_> {
    pub(crate) fn decision(&self) -> &PoolMutationPlan {
        &self.plan
    }

    pub(crate) fn pool(&self) -> &PoolMap {
        self.pool
    }

    /// The multi-index candidate insertion is the only library operation that
    /// can still report failure. It runs before every other physical change;
    /// success consumes the exclusive proof and all remaining projection
    /// writes are total HashMap/HashSet operations.
    pub(crate) fn apply(self) -> Result<(), PoolMutationFault> {
        self.pool.apply_prepared(self.plan)
    }
}

/// The canonical out-point memberships published for one accepted entry.
///
/// Transaction and dep-group resolution may expose the same out-point more
/// than once. The reverse indexes are sets, so validation, publication and
/// removal must all operate on this same normalized keyset. Replaying the raw
/// iterator during removal would otherwise try to delete one logical
/// membership twice and turn a valid transaction into an authoritative
/// invariant failure.
#[derive(Debug)]
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
#[derive(Clone, Copy, Debug, Default)]
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
        self.stats
            .pending_count
            .saturating_add(self.stats.gap_count)
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
    pub(crate) fn prepare_mutation(
        &mut self,
        mut candidate: TxEntry,
        status: Status,
        mandatory_removal: &[ProposalShortId],
        max_tx_pool_size: usize,
        max_resident_size: usize,
    ) -> Result<PreparedPoolMutation<'_>, PoolMutationPlanningError> {
        let candidate_id = candidate.proposal_short_id();
        let candidate_hash = candidate.transaction().hash();
        if self.get_by_hash(&candidate_hash).is_some() {
            return Err(Reject::Duplicated(candidate_hash).into());
        }
        if self.get_by_id(&candidate_id).is_some() {
            return Err(Reject::Full(format!(
                "proposal short-id collision while planning {candidate_hash}"
            ))
            .into());
        }

        let mut removed = HashSet::with_capacity(mandatory_removal.len());
        let mut removals = Vec::with_capacity(mandatory_removal.len());
        for id in mandatory_removal {
            if !removed.insert(id.clone()) {
                continue;
            }
            let entry = self.get_by_id(id).ok_or(PoolMutationFault::MissingEntry(
                "bounded RBF closure victim",
            ))?;
            let links = self
                .links
                .get(id)
                .ok_or(PoolMutationFault::MissingEntry("bounded RBF closure links"))?;
            removals.push(PlannedRemoval {
                id: id.clone(),
                hash: entry.hash.clone(),
                status: entry.status,
                cause: RemovalCause::Replacement,
                entry: entry.inner.clone(),
                links: links.clone(),
            });
        }
        if removed.len() > MAX_POOL_MUTATION_CANDIDATES {
            return Err(Reject::Full(format!(
                "pool mutation exceeds the per-transition limit of {MAX_POOL_MUTATION_CANDIDATES}"
            ))
            .into());
        }

        self.pre_validate_entry_inputs_excluding(&candidate, &removed)?;
        if self.has_surviving_causal_child(&candidate, &removed) {
            return Err(Reject::Malformed(
                "pool".to_string(),
                "ordinary admission would introduce a late causal parent".to_string(),
            )
            .into());
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
                .ok_or(PoolMutationFault::MissingEntry("mandatory victim totals"))?
                .inner;
            total_size =
                total_size
                    .checked_sub(entry.size)
                    .ok_or(PoolMutationFault::ProjectionMismatch(
                        "serialized total does not cover mandatory victim",
                    ))?;
            total_resident = total_resident.checked_sub(entry.resident_size()).ok_or(
                PoolMutationFault::ProjectionMismatch(
                    "resident total does not cover mandatory victim",
                ),
            )?;
            total_cycles = total_cycles.checked_sub(entry.cycles).ok_or(
                PoolMutationFault::ProjectionMismatch(
                    "cycle total does not cover mandatory victim",
                ),
            )?;
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
        self.adjust_virtual_ancestors(&removed, &removed, &mut overlay)?;
        for ancestor in &candidate_ancestors {
            if !overlay.contains_key(ancestor) {
                let entry = self
                    .get_by_id(ancestor)
                    .ok_or(PoolMutationFault::MissingEntry("candidate ancestor"))?;
                overlay.insert(ancestor.clone(), (entry.inner.clone(), entry.status));
            }
            let item = overlay
                .get_mut(ancestor)
                .ok_or(PoolMutationFault::MissingEntry(
                    "candidate ancestor overlay",
                ))?;
            item.0
                .add_descendant_weight(&candidate)
                .map_err(PoolMutationFault::from)?;
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
                ))
                .into());
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
                ))
                .into());
            }
            let next_removed = removed
                .iter()
                .cloned()
                .chain(closure.iter().cloned())
                .collect::<HashSet<_>>();
            let closure_set = closure.iter().cloned().collect::<HashSet<_>>();
            self.adjust_virtual_ancestors(&closure_set, &next_removed, &mut overlay)?;
            for id in closure {
                if !removed.insert(id.clone()) {
                    continue;
                }
                let current = self.get_by_id(&id).ok_or(PoolMutationFault::MissingEntry(
                    "virtual eviction candidate",
                ))?;
                let links = self.links.get(&id).ok_or(PoolMutationFault::MissingEntry(
                    "virtual eviction candidate links",
                ))?;
                let entry = overlay.get(&id).map_or(&current.inner, |(entry, _)| entry);
                total_size = total_size.checked_sub(entry.size).ok_or(
                    PoolMutationFault::ProjectionMismatch(
                        "serialized total does not cover virtual eviction",
                    ),
                )?;
                total_resident = total_resident.checked_sub(entry.resident_size()).ok_or(
                    PoolMutationFault::ProjectionMismatch(
                        "resident total does not cover virtual eviction",
                    ),
                )?;
                total_cycles = total_cycles.checked_sub(entry.cycles).ok_or(
                    PoolMutationFault::ProjectionMismatch(
                        "cycle total does not cover virtual eviction",
                    ),
                )?;
                removals.push(PlannedRemoval {
                    id,
                    hash: current.hash.clone(),
                    status: current.status,
                    cause: RemovalCause::SizeLimit,
                    entry: current.inner.clone(),
                    links: links.clone(),
                });
            }
        }

        let candidate_edges = EntryOutPointEdges::from_entry(&candidate);
        let survivor_updates = overlay
            .into_iter()
            .filter(|(id, _)| id != &candidate_id && !removed.contains(id))
            .map(|(id, (entry, _))| (id, entry))
            .collect::<Vec<_>>();
        let post_stats =
            self.project_post_stats(&removals, status, total_size, total_resident, total_cycles)?;
        self.validate_prepared_projection(
            &candidate,
            &candidate_edges,
            &candidate_parents,
            &removals,
            &survivor_updates,
            &removed,
        )?;

        Ok(PreparedPoolMutation {
            pool: self,
            plan: PoolMutationPlan {
                candidate,
                status,
                removals,
                candidate_parents,
                candidate_edges,
                survivor_updates,
                post_stats,
            },
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
    ) -> Result<(HashSet<ProposalShortId>, HashSet<ProposalShortId>), PoolMutationPlanningError>
    {
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
            return Err(Reject::ExceededMaximumAncestorsCount.into());
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
    ) -> Result<(), PoolMutationFault> {
        for id in removed_now {
            let removed_entry = overlay
                .get(id)
                .map(|(entry, _)| entry.clone())
                .or_else(|| self.get_by_id(id).map(|entry| entry.inner.clone()))
                .ok_or(PoolMutationFault::MissingEntry("virtual removal"))?;
            for ancestor in self.links.calc_ancestors(id) {
                if removed_after.contains(&ancestor) {
                    continue;
                }
                if !overlay.contains_key(&ancestor) {
                    let entry =
                        self.get_by_id(&ancestor)
                            .ok_or(PoolMutationFault::MissingEntry(
                                "surviving virtual ancestor",
                            ))?;
                    overlay.insert(ancestor.clone(), (entry.inner.clone(), entry.status));
                }
                let item = overlay
                    .get_mut(&ancestor)
                    .ok_or(PoolMutationFault::MissingEntry(
                        "surviving virtual ancestor overlay",
                    ))?;
                item.0.sub_descendant_weight(&removed_entry)?;
            }
        }
        Ok(())
    }

    pub(crate) fn prepare_removals(
        &mut self,
        ordered_ids: &[ProposalShortId],
    ) -> Result<Option<PreparedPoolRemoval<'_>>, PoolMutationFault> {
        let mut removed = HashSet::with_capacity(ordered_ids.len());
        let mut removals = Vec::with_capacity(ordered_ids.len());
        for id in ordered_ids {
            if removed.contains(id) {
                continue;
            }
            let Some(current) = self.get_by_id(id) else {
                continue;
            };
            let links = self
                .links
                .get(id)
                .ok_or(PoolMutationFault::MissingEntry("removal links"))?;
            removed.insert(id.clone());
            removals.push(ProjectionRemoval {
                id: id.clone(),
                hash: current.hash.clone(),
                status: current.status,
                entry: current.inner.clone(),
                links: links.clone(),
            });
        }
        if removals.is_empty() {
            return Ok(None);
        }

        let mut overlay = HashMap::<ProposalShortId, (TxEntry, Status)>::new();
        self.adjust_virtual_ancestors(&removed, &removed, &mut overlay)?;
        for removal in &removals {
            for descendant in self.links.calc_descendants(&removal.id) {
                if removed.contains(&descendant) {
                    continue;
                }
                if !overlay.contains_key(&descendant) {
                    let current =
                        self.get_by_id(&descendant)
                            .ok_or(PoolMutationFault::MissingEntry(
                                "surviving removal descendant",
                            ))?;
                    overlay.insert(descendant.clone(), (current.inner.clone(), current.status));
                }
                let update =
                    overlay
                        .get_mut(&descendant)
                        .ok_or(PoolMutationFault::MissingEntry(
                            "surviving descendant overlay",
                        ))?;
                update.0.sub_ancestor_weight(&removal.entry)?;
            }
        }
        let survivor_updates = overlay
            .into_iter()
            .filter(|(id, _)| !removed.contains(id))
            .map(|(id, (entry, _))| (id, entry))
            .collect::<Vec<_>>();

        let mut post_stats = self.stats;
        for removal in &removals {
            post_stats.total_tx_size = post_stats
                .total_tx_size
                .checked_sub(removal.entry.size)
                .ok_or(PoolMutationFault::ProjectionMismatch(
                    "removal serialized total",
                ))?;
            post_stats.total_tx_resident_size = post_stats
                .total_tx_resident_size
                .checked_sub(removal.entry.resident_size())
                .ok_or(PoolMutationFault::ProjectionMismatch(
                    "removal resident total",
                ))?;
            post_stats.total_tx_cycles = post_stats
                .total_tx_cycles
                .checked_sub(removal.entry.cycles)
                .ok_or(PoolMutationFault::ProjectionMismatch("removal cycle total"))?;
            let count = match removal.status {
                Status::Pending => &mut post_stats.pending_count,
                Status::Gap => &mut post_stats.gap_count,
                Status::Proposed => &mut post_stats.proposed_count,
            };
            *count = count
                .checked_sub(1)
                .ok_or(PoolMutationFault::ProjectionMismatch(
                    "removal status count",
                ))?;
            self.validate_removal_record(
                &removal.id,
                &removal.hash,
                removal.status,
                &removal.entry,
                &removal.links,
                &removed,
                false,
            )?;
        }
        for (id, _) in &survivor_updates {
            if removed.contains(id) || self.get_by_id(id).is_none() {
                return Err(PoolMutationFault::MissingEntry(
                    "removal survivor projection",
                ));
            }
        }
        let projected_len = post_stats
            .pending_count
            .checked_add(post_stats.gap_count)
            .and_then(|count| count.checked_add(post_stats.proposed_count))
            .ok_or(PoolMutationFault::CounterOverflow(
                "removal projected membership count",
            ))?;
        let expected_len = self.entries.len().checked_sub(removals.len()).ok_or(
            PoolMutationFault::ProjectionMismatch("removal membership count"),
        )?;
        if projected_len != expected_len {
            return Err(PoolMutationFault::ProjectionMismatch(
                "removal status counts disagree with membership",
            ));
        }

        Ok(Some(PreparedPoolRemoval {
            pool: self,
            plan: PoolRemovalPlan {
                removed,
                removals,
                survivor_updates,
                post_stats,
            },
        }))
    }

    fn project_post_stats(
        &self,
        removals: &[PlannedRemoval],
        candidate_status: Status,
        total_tx_size: usize,
        total_tx_resident_size: usize,
        total_tx_cycles: Cycle,
    ) -> Result<PoolStats, PoolMutationFault> {
        let mut projected = self.stats;
        for removal in removals {
            let count = match removal.status {
                Status::Pending => &mut projected.pending_count,
                Status::Gap => &mut projected.gap_count,
                Status::Proposed => &mut projected.proposed_count,
            };
            *count = count
                .checked_sub(1)
                .ok_or(PoolMutationFault::ProjectionMismatch(
                    "accepted status count underflow",
                ))?;
        }
        let count = match candidate_status {
            Status::Pending => &mut projected.pending_count,
            Status::Gap => &mut projected.gap_count,
            Status::Proposed => &mut projected.proposed_count,
        };
        *count = count
            .checked_add(1)
            .ok_or(PoolMutationFault::CounterOverflow("accepted status count"))?;
        projected.total_tx_size = total_tx_size;
        projected.total_tx_resident_size = total_tx_resident_size;
        projected.total_tx_cycles = total_tx_cycles;

        let expected_len = self
            .entries
            .len()
            .checked_sub(removals.len())
            .and_then(|len| len.checked_add(1))
            .ok_or(PoolMutationFault::ProjectionMismatch(
                "accepted membership count",
            ))?;
        let projected_len = projected
            .pending_count
            .checked_add(projected.gap_count)
            .and_then(|count| count.checked_add(projected.proposed_count))
            .ok_or(PoolMutationFault::CounterOverflow(
                "accepted projected membership count",
            ))?;
        if projected_len != expected_len {
            return Err(PoolMutationFault::ProjectionMismatch(
                "accepted status counts disagree with membership",
            ));
        }
        Ok(projected)
    }

    fn validate_prepared_projection(
        &self,
        candidate: &TxEntry,
        candidate_edges: &EntryOutPointEdges,
        candidate_parents: &HashSet<ProposalShortId>,
        removals: &[PlannedRemoval],
        survivor_updates: &[(ProposalShortId, TxEntry)],
        removed: &HashSet<ProposalShortId>,
    ) -> Result<(), PoolMutationFault> {
        let candidate_id = candidate.proposal_short_id();
        let candidate_hash = candidate.transaction().hash();
        if self.get_by_hash(&candidate_hash).is_some() || self.get_by_id(&candidate_id).is_some() {
            return Err(PoolMutationFault::CandidateIdentityDrift);
        }
        for parent in candidate_parents {
            let links = self
                .links
                .get(parent)
                .ok_or(PoolMutationFault::MissingEntry("candidate parent links"))?;
            if removed.contains(parent) || links.children.contains(&candidate_id) {
                return Err(PoolMutationFault::ProjectionMismatch(
                    "candidate parent projection",
                ));
            }
        }
        for input in &candidate_edges.inputs {
            if self
                .out_point_index
                .get_input_ref(input)
                .is_some_and(|owner| !removed.contains(owner))
            {
                return Err(PoolMutationFault::ProjectionMismatch(
                    "candidate input is occupied by a surviving owner",
                ));
            }
        }
        for (id, _) in survivor_updates {
            if removed.contains(id) || self.get_by_id(id).is_none() {
                return Err(PoolMutationFault::MissingEntry(
                    "survivor projection update",
                ));
            }
        }
        for removal in removals {
            self.validate_removal_record(
                &removal.id,
                &removal.hash,
                removal.status,
                &removal.entry,
                &removal.links,
                removed,
                true,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_removal_record(
        &self,
        id: &ProposalShortId,
        hash: &Byte32,
        status: Status,
        entry: &TxEntry,
        links: &TxLinks,
        removed: &HashSet<ProposalShortId>,
        require_closed_descendants: bool,
    ) -> Result<(), PoolMutationFault> {
        let current = self
            .get_by_id(id)
            .ok_or(PoolMutationFault::MissingEntry("prepared removal"))?;
        if &current.hash != hash
            || current.status != status
            || current.inner.transaction().hash() != entry.transaction().hash()
        {
            return Err(PoolMutationFault::ProjectionMismatch(
                "prepared removal identity",
            ));
        }
        let current_links = self
            .links
            .get(id)
            .ok_or(PoolMutationFault::MissingEntry("prepared removal links"))?;
        if current_links != links
            || (require_closed_descendants
                && links.children.iter().any(|child| !removed.contains(child)))
        {
            return Err(PoolMutationFault::ProjectionMismatch(
                "prepared removal graph closure",
            ));
        }
        for parent in &links.parents {
            if !self
                .links
                .get(parent)
                .is_some_and(|parent_links| parent_links.children.contains(id))
            {
                return Err(PoolMutationFault::ProjectionMismatch(
                    "prepared removal parent symmetry",
                ));
            }
        }
        for child in &links.children {
            if !self
                .links
                .get(child)
                .is_some_and(|child_links| child_links.parents.contains(id))
            {
                return Err(PoolMutationFault::ProjectionMismatch(
                    "prepared removal child symmetry",
                ));
            }
        }
        let edges = EntryOutPointEdges::from_entry(entry);
        if edges
            .inputs
            .iter()
            .any(|input| self.out_point_index.get_input_ref(input) != Some(id))
            || edges.deps.iter().any(|dep| {
                !self
                    .out_point_index
                    .get_deps_ref(dep)
                    .is_some_and(|ids| ids.contains(id))
            })
        {
            return Err(PoolMutationFault::ProjectionMismatch(
                "prepared removal out-point projection",
            ));
        }
        let expected_headers = entry
            .transaction()
            .header_deps()
            .into_iter()
            .collect::<Vec<_>>();
        let actual_headers = self.out_point_index.header_deps.get(id);
        if (expected_headers.is_empty() && actual_headers.is_some())
            || (!expected_headers.is_empty()
                && actual_headers.map(Vec::as_slice) != Some(expected_headers.as_slice()))
        {
            return Err(PoolMutationFault::ProjectionMismatch(
                "prepared removal header projection",
            ));
        }
        Ok(())
    }

    fn apply_prepared(&mut self, plan: PoolMutationPlan) -> Result<(), PoolMutationFault> {
        let PoolMutationPlan {
            candidate,
            status,
            removals,
            candidate_parents,
            candidate_edges,
            survivor_updates,
            post_stats,
        } = plan;
        let candidate_id = candidate.proposal_short_id();
        let candidate_hash = candidate.transaction().hash();
        let candidate_headers = candidate
            .transaction()
            .header_deps()
            .into_iter()
            .collect::<Vec<_>>();
        let score = candidate.as_score_key();
        let evict_key = candidate.as_evict_key();
        if self
            .entries
            .try_insert(PoolEntry {
                hash: candidate_hash,
                id: candidate_id.clone(),
                score,
                status,
                inner: candidate,
                evict_key,
            })
            .is_err()
        {
            return Err(PoolMutationFault::CandidateIdentityDrift);
        }

        for planned in &removals {
            let _ = self.entries.remove_by_id(&planned.id);
            for parent in &planned.links.parents {
                let _ = self.links.remove_child(parent, &planned.id);
            }
            for child in &planned.links.children {
                let _ = self.links.remove_parent(child, &planned.id);
            }
            let _ = self.links.remove(&planned.id);

            let edges = EntryOutPointEdges::from_entry(&planned.entry);
            for input in edges.inputs {
                let _ = self.out_point_index.remove_input(&input);
            }
            for dep in edges.deps {
                self.out_point_index.delete_txid_by_dep(dep, &planned.id);
            }
            self.out_point_index.header_deps.remove(&planned.id);
        }

        for (id, entry) in survivor_updates {
            let score = entry.as_score_key();
            let evict_key = entry.as_evict_key();
            let _ = self.entries.modify_by_id(&id, |current| {
                current.inner = entry;
                current.score = score;
                current.evict_key = evict_key;
            });
        }
        for parent in &candidate_parents {
            let _ = self.links.add_child(parent, candidate_id.clone());
        }
        self.links.add_link(
            candidate_id.clone(),
            TxLinks {
                parents: candidate_parents,
                children: HashSet::new(),
            },
        );
        for input in candidate_edges.inputs {
            self.out_point_index
                .inputs
                .insert(input, candidate_id.clone());
        }
        for dep in candidate_edges.deps {
            self.out_point_index.insert_deps(dep, candidate_id.clone());
        }
        if !candidate_headers.is_empty() {
            self.out_point_index
                .header_deps
                .insert(candidate_id, candidate_headers);
        }
        self.stats = post_stats;
        self.publish_stats_metrics();
        Ok(())
    }

    fn apply_prepared_removal(&mut self, plan: PoolRemovalPlan) -> Vec<TxEntry> {
        let PoolRemovalPlan {
            removals,
            survivor_updates,
            post_stats,
            ..
        } = plan;
        let mut removed_entries = Vec::with_capacity(removals.len());
        for removal in removals {
            let _ = self.entries.remove_by_id(&removal.id);
            for parent in &removal.links.parents {
                let _ = self.links.remove_child(parent, &removal.id);
            }
            for child in &removal.links.children {
                let _ = self.links.remove_parent(child, &removal.id);
            }
            let _ = self.links.remove(&removal.id);
            let edges = EntryOutPointEdges::from_entry(&removal.entry);
            for input in edges.inputs {
                let _ = self.out_point_index.remove_input(&input);
            }
            for dep in edges.deps {
                self.out_point_index.delete_txid_by_dep(dep, &removal.id);
            }
            self.out_point_index.header_deps.remove(&removal.id);
            removed_entries.push(removal.entry);
        }
        for (id, entry) in survivor_updates {
            let score = entry.as_score_key();
            let evict_key = entry.as_evict_key();
            let _ = self.entries.modify_by_id(&id, |current| {
                current.inner = entry;
                current.score = score;
                current.evict_key = evict_key;
            });
        }
        self.stats = post_stats;
        self.publish_stats_metrics();
        removed_entries
    }

    /// Internal instrumentation inserts through the same immutable Plan and
    /// total Apply as ordinary admission. The permissive child-first graph
    /// constructor is isolated in the dedicated test module.
    #[cfg(feature = "internal")]
    pub(crate) fn plug_entry(&mut self, entry: TxEntry, status: Status) -> Result<bool, Reject> {
        if self.get_by_hash(&entry.transaction().hash()).is_some() {
            return Ok(false);
        }
        let prepared = self
            .prepare_mutation(entry, status, &[], usize::MAX, usize::MAX)
            .map_err(|error| match error {
                PoolMutationPlanningError::Policy(reject) => reject,
                PoolMutationPlanningError::Fault(fault) => {
                    Reject::Internal(format!("accepted-pool planning fault: {fault:?}"))
                }
            })?;
        prepared
            .apply()
            .map_err(|fault| Reject::Internal(format!("accepted-pool apply fault: {fault:?}")))?;
        Ok(true)
    }

    pub(crate) fn prepare_status_changes(
        &mut self,
        transitions: Vec<(ProposalShortId, Status)>,
    ) -> Result<PreparedPoolStatus<'_>, PoolMutationFault> {
        let mut seen = HashSet::with_capacity(transitions.len());
        let mut post_stats = self.stats;
        for (id, target) in &transitions {
            if !seen.insert(id.clone()) {
                return Err(PoolMutationFault::ProjectionMismatch(
                    "duplicate accepted status transition",
                ));
            }
            let current = self.get_by_id(id).ok_or(PoolMutationFault::MissingEntry(
                "accepted status transition",
            ))?;
            if current.status == *target {
                return Err(PoolMutationFault::ProjectionMismatch(
                    "redundant accepted status transition",
                ));
            }
            let old_count = match current.status {
                Status::Pending => &mut post_stats.pending_count,
                Status::Gap => &mut post_stats.gap_count,
                Status::Proposed => &mut post_stats.proposed_count,
            };
            *old_count = old_count
                .checked_sub(1)
                .ok_or(PoolMutationFault::ProjectionMismatch(
                    "accepted status count underflow",
                ))?;
            let new_count = match target {
                Status::Pending => &mut post_stats.pending_count,
                Status::Gap => &mut post_stats.gap_count,
                Status::Proposed => &mut post_stats.proposed_count,
            };
            *new_count = new_count
                .checked_add(1)
                .ok_or(PoolMutationFault::CounterOverflow(
                    "accepted status count overflow",
                ))?;
        }
        let projected_len = post_stats
            .pending_count
            .checked_add(post_stats.gap_count)
            .and_then(|count| count.checked_add(post_stats.proposed_count))
            .ok_or(PoolMutationFault::CounterOverflow(
                "accepted status projected membership",
            ))?;
        if projected_len != self.entries.len() {
            return Err(PoolMutationFault::ProjectionMismatch(
                "accepted status counts disagree with membership",
            ));
        }
        Ok(PreparedPoolStatus {
            pool: self,
            plan: PoolStatusPlan {
                transitions,
                post_stats,
            },
        })
    }

    pub(crate) fn remove_entry(
        &mut self,
        id: &ProposalShortId,
    ) -> Result<Option<TxEntry>, PoolMutationFault> {
        let Some(prepared) = self.prepare_removals(std::slice::from_ref(id))? else {
            return Ok(None);
        };
        let mut removed = prepared.apply();
        Ok(removed.pop())
    }

    pub(crate) fn remove_entry_and_descendants(
        &mut self,
        id: &ProposalShortId,
    ) -> Result<Vec<TxEntry>, PoolMutationFault> {
        let roots = HashSet::from([id.clone()]);
        let ordered = match self.conflict_closure(&roots, self.len()) {
            ConflictClosure::Complete { removal, .. } => removal,
            ConflictClosure::Exceeded { .. } => {
                return Err(PoolMutationFault::ProjectionMismatch(
                    "accepted descendant closure exceeds membership",
                ));
            }
        };
        let Some(prepared) = self.prepare_removals(&ordered)? else {
            return Ok(Vec::new());
        };
        Ok(prepared.apply())
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
    ) -> Result<Vec<ConflictEntry>, PoolMutationFault> {
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
            let entries = self.remove_entry_and_descendants(&id)?;
            for entry in entries {
                let reject = Reject::Resolve(OutPointError::InvalidHeader(blk_hash.to_owned()));
                conflicts.push((entry, reject));
            }
        }
        Ok(conflicts)
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

    pub(crate) fn resolve_conflict(
        &mut self,
        tx: &TransactionView,
    ) -> Result<Vec<ConflictEntry>, PoolMutationFault> {
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
            let entries = self.remove_entry_and_descendants(&id)?;
            if !entries.is_empty() {
                let reject = Reject::Resolve(OutPointError::Dead(out_point));
                let rejects = std::iter::repeat_n(reject, entries.len());
                conflicts.extend(entries.into_iter().zip(rejects));
            }
        }

        Ok(conflicts)
    }

    pub(crate) fn estimate_fee_rate(
        &self,
        target_blocks: std::num::NonZeroUsize,
        max_block_bytes: usize,
        max_block_cycles: Cycle,
        min_fee_rate: FeeRate,
    ) -> FeeRate {
        let mut remaining_blocks = target_blocks.get();
        let iter = self.entries.iter_by_score().rev();
        let mut current_block_bytes = 0usize;
        let mut current_block_cycles: Cycle = 0;
        for entry in iter {
            current_block_bytes = current_block_bytes.saturating_add(entry.inner.size);
            current_block_cycles = current_block_cycles.saturating_add(entry.inner.cycles);
            if current_block_bytes >= max_block_bytes || current_block_cycles >= max_block_cycles {
                remaining_blocks = match remaining_blocks.checked_sub(1) {
                    Some(0) => return entry.inner.fee_rate(),
                    Some(remaining) => remaining,
                    None => return min_fee_rate,
                };
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
        let mut parents =
            HashSet::with_capacity(tx.inputs().len().saturating_add(tx.cell_deps().len()));

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
    /// the prepared Apply path).
    fn apply_ancestor_weights(
        &self,
        entry: &mut TxEntry,
        ancestors: &HashSet<ProposalShortId>,
    ) -> Result<(), PoolMutationFault> {
        for ancestor_id in ancestors {
            let ancestor = self
                .get_by_id(ancestor_id)
                .ok_or(PoolMutationFault::MissingEntry("accepted ancestor link"))?;
            entry.add_ancestor_weight(&ancestor.inner)?;
        }
        Ok(())
    }

    fn publish_stats_metrics(&self) {
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
}
