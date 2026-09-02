mod eviction;
mod independent;
mod policy;
mod rbf;

use policy::{PolicyContext, PolicyMode};

pub(in crate::authority::plan) use independent::{
    IndependentMembershipChange, IndependentMembershipOutcome, PreparedIndependentMembership,
    has_membership_relation_coupling, prepare_classified_ordinary_membership,
    prepare_independent_membership,
};

use super::TxPoolAuthority;
use crate::authority::{
    dependency::{ObservedAcceptedConsumers, ObservedDependencyConsumerRead},
    rejection::{ComponentLimitKind, MembershipReject},
    resources::AcceptedCost,
    scheduler::StagedIngressVisibility,
    shard::{
        AUTHORITY_SHARD_COUNT, OwnerEntryKind, OwnerShardRemovalRevision, ShardReadSupport,
        ShardWriteSupport, ShardedAcceptedReadGuard, ShardedOwnerMap, ShardedOwnerWriteCut,
    },
    state::{
        AcceptedEntry, AcceptedStatus, Arrival, DependencyKey, EntryVersion, OwnedTx,
        PreAcceptedEntry, RawTxHash, ReplacementHistoryEntry,
    },
};
use crate::component::sort_key::AncestorsScoreSortKey;
use ckb_types::{
    core::{Capacity, FeeRate, tx_pool::get_transaction_weight},
    packed::OutPoint,
    prelude::Unpack,
};
use std::{
    cmp::Reverse,
    collections::{BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque},
    ops::Deref,
};

#[derive(Clone, Copy, Debug)]
enum ReplacementPolicy {
    Disabled,
    Enabled { minimum_rate: FeeRate },
}

#[derive(Clone, Copy, Debug)]
pub(in crate::authority) struct MembershipConfig {
    max_ancestors: usize,
    max_component: usize,
    replacement: ReplacementPolicy,
}

impl MembershipConfig {
    pub(in crate::authority) fn from_runtime(
        max_ancestors: usize,
        max_component: usize,
        minimum_replacement_rate: Option<FeeRate>,
    ) -> Self {
        let replacement = minimum_replacement_rate
            .map_or(ReplacementPolicy::Disabled, |minimum_rate| {
                ReplacementPolicy::Enabled { minimum_rate }
            });
        Self {
            max_ancestors,
            max_component,
            replacement,
        }
    }

    pub(super) fn max_component(self) -> usize {
        self.max_component
    }

    pub(in crate::authority) fn minimum_replacement_rate(self) -> Option<FeeRate> {
        match self.replacement {
            ReplacementPolicy::Disabled => None,
            ReplacementPolicy::Enabled { minimum_rate } => Some(minimum_rate),
        }
    }
}

/// Canonical set of Accepted owners removed by one administrative or chain
/// transition. The owning vector is sorted and duplicate-free at its sealed
/// constructor, so every set lookup has the same deterministic semantics and
/// no caller can accidentally pass traversal order to a binary search.
pub(super) struct AcceptedRemovalSet {
    hashes: Vec<RawTxHash>,
}

impl AcceptedRemovalSet {
    pub(super) fn try_from_vec(mut hashes: Vec<RawTxHash>) -> Result<Self, super::PlanError> {
        hashes.sort_unstable();
        if hashes
            .array_windows::<2>()
            .any(|[left, right]| left == right)
        {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        Ok(Self { hashes })
    }

    pub(super) fn contains(&self, hash: &RawTxHash) -> bool {
        self.hashes.binary_search(hash).is_ok()
    }

    pub(super) fn iter(&self) -> std::slice::Iter<'_, RawTxHash> {
        self.hashes.iter()
    }

    pub(super) fn len(&self) -> usize {
        self.hashes.len()
    }
}

/// Exact Accepted descendant closure captured without owning membership
/// policy. The caller-selected postorder is kept separate from the routed
/// child-row snapshots that later seal the administrative projection.
pub(super) struct AdministrativeDescendantClosure {
    ordered: Vec<RawTxHash>,
    child_rows: Vec<(RawTxHash, HashSet<RawTxHash>)>,
}

pub(super) struct AdministrativeClosureWitness {
    child_rows: Vec<(RawTxHash, HashSet<RawTxHash>)>,
}

impl AdministrativeDescendantClosure {
    pub(super) fn into_parts(self) -> (Vec<RawTxHash>, AdministrativeClosureWitness) {
        (
            self.ordered,
            AdministrativeClosureWitness {
                child_rows: self.child_rows,
            },
        )
    }
}

impl AdministrativeClosureWitness {
    /// Bind the traversal to the exact rows captured by the canonical
    /// administrative projection. `prepare_chain_projection` deliberately
    /// permits surviving children for chain transitions, so this additional
    /// seal is what makes a local/expiry removal descendant-complete.
    pub(super) fn matches_projection(
        &self,
        removals: &AcceptedRemovalSet,
        projection: &ProjectionDelta,
    ) -> bool {
        self.child_rows.len() == removals.len()
            && self.child_rows.iter().all(|(hash, children)| {
                removals.contains(hash)
                    && children.iter().all(|child| removals.contains(child))
                    && projection
                        .prestate
                        .children
                        .binary_search_by(|(candidate, _)| candidate.cmp(hash))
                        .ok()
                        .and_then(|position| projection.prestate.children.get(position))
                        .is_some_and(|(_, captured)| captured.as_ref() == Some(children))
            })
    }
}

struct BoundedDescendantPostorder {
    ordered: Vec<RawTxHash>,
    child_rows: Vec<(RawTxHash, HashSet<RawTxHash>)>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct StatusCounts {
    pub(in crate::authority) pending: usize,
    pub(in crate::authority) gap: usize,
    pub(in crate::authority) proposed: usize,
}

#[cfg(test)]
impl StatusCounts {
    pub(in crate::authority) fn checked_add(self, status: AcceptedStatus) -> Option<Self> {
        let mut next = self;
        let count = next.for_status_mut(status);
        *count = count.checked_add(1)?;
        Some(next)
    }

    fn for_status_mut(&mut self, status: AcceptedStatus) -> &mut usize {
        match status {
            AcceptedStatus::Pending => &mut self.pending,
            AcceptedStatus::Gap => &mut self.gap,
            AcceptedStatus::Proposed => &mut self.proposed,
        }
    }
}

#[derive(Debug)]
pub(in crate::authority) struct MembershipProjection {
    entries: ShardedOwnerMap,
}

impl MembershipProjection {
    pub(super) fn for_entries(entries: &ShardedOwnerMap) -> Self {
        Self {
            entries: entries.clone(),
        }
    }

    fn shard<K: std::hash::Hash>(&self, domain: &'static [u8], key: &K) -> usize {
        self.entries.layout.router.shard(domain, key)
    }

    fn owner_shard(&self, key: &RawTxHash) -> usize {
        self.entries.owner_shard(key)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct AncestorAggregate {
    pub(in crate::authority) entries: usize,
    pub(in crate::authority) serialized_bytes: usize,
    pub(in crate::authority) cycles: u64,
    pub(in crate::authority) fee: Capacity,
}

impl AncestorAggregate {
    pub(super) fn one(entry: &AcceptedEntry) -> Self {
        let cost = entry.proof.metrics().cost;
        Self {
            entries: 1,
            serialized_bytes: cost.serialized_bytes,
            cycles: cost.cycles,
            fee: entry.proof.metrics().fee,
        }
    }

    fn checked_add_entry(self, entry: &AcceptedEntry) -> Option<Self> {
        let cost = entry.proof.metrics().cost;
        Some(Self {
            entries: self.entries.checked_add(1)?,
            serialized_bytes: self.serialized_bytes.checked_add(cost.serialized_bytes)?,
            cycles: self.cycles.checked_add(cost.cycles)?,
            fee: self.fee.safe_add(entry.proof.metrics().fee).ok()?,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct DescendantAggregate {
    pub(in crate::authority) entries: usize,
    pub(in crate::authority) serialized_bytes: usize,
    pub(in crate::authority) cycles: u64,
    pub(in crate::authority) fee: Capacity,
}

impl DescendantAggregate {
    pub(super) fn one(entry: &AcceptedEntry) -> Self {
        let cost = entry.proof.metrics().cost;
        Self {
            entries: 1,
            serialized_bytes: cost.serialized_bytes,
            cycles: cost.cycles,
            fee: entry.proof.metrics().fee,
        }
    }

    fn checked_add_entry(self, entry: &AcceptedEntry) -> Option<Self> {
        self.checked_add(Self::one(entry))
    }

    fn checked_sub_entry(self, entry: &AcceptedEntry) -> Option<Self> {
        self.checked_sub(Self::one(entry))
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            serialized_bytes: self.serialized_bytes.checked_add(other.serialized_bytes)?,
            cycles: self.cycles.checked_add(other.cycles)?,
            fee: self.fee.safe_add(other.fee).ok()?,
        })
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(other.entries)?,
            serialized_bytes: self.serialized_bytes.checked_sub(other.serialized_bytes)?,
            cycles: self.cycles.checked_sub(other.cycles)?,
            fee: self.fee.safe_sub(other.fee).ok()?,
        })
    }
}

/// Canonical accepted-package order shared by template selection and RPC
/// troubleshooting. It is a payload-free derived index: membership Plan
/// computes it from exact ancestor aggregates and Apply publishes it with the
/// owner/relation change that made the aggregate true.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct AcceptedOrderKey {
    score: AncestorsScoreSortKey,
    arrival: Arrival,
    hash: RawTxHash,
}

impl AcceptedOrderKey {
    pub(in crate::authority) fn new(entry: &AcceptedEntry, aggregate: AncestorAggregate) -> Self {
        Self {
            score: AncestorsScoreSortKey {
                fee: entry.proof.metrics().fee,
                weight: get_transaction_weight(
                    entry.proof.metrics().cost.serialized_bytes,
                    entry.proof.metrics().cost.cycles,
                ),
                ancestors_fee: aggregate.fee,
                ancestors_weight: get_transaction_weight(
                    aggregate.serialized_bytes,
                    aggregate.cycles,
                ),
            },
            arrival: entry.record.arrival,
            hash: entry.record.identity.raw.clone(),
        }
    }

    pub(in crate::authority) fn hash(&self) -> &RawTxHash {
        &self.hash
    }

    pub(in crate::authority) fn score(&self) -> &AncestorsScoreSortKey {
        &self.score
    }

    pub(in crate::authority) fn arrival(&self) -> Arrival {
        self.arrival
    }
}

impl Ord for AcceptedOrderKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.hash.cmp(&self.hash))
    }
}

impl PartialOrd for AcceptedOrderKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::authority) struct EvictionOrderKey {
    pub(in crate::authority) status: AcceptedStatus,
    pub(in crate::authority) fee_rate: FeeRate,
    pub(in crate::authority) descendants_count: usize,
    pub(in crate::authority) arrival: Arrival,
    pub(in crate::authority) hash: RawTxHash,
}

impl EvictionOrderKey {
    pub(in crate::authority) fn new(entry: &AcceptedEntry, aggregate: DescendantAggregate) -> Self {
        let AcceptedCost {
            serialized_bytes,
            cycles,
            ..
        } = entry.proof.metrics().cost;
        let self_rate = FeeRate::calculate(
            entry.proof.metrics().fee,
            get_transaction_weight(serialized_bytes, cycles),
        );
        let descendants_rate = FeeRate::calculate(
            aggregate.fee,
            get_transaction_weight(aggregate.serialized_bytes, aggregate.cycles),
        );
        Self {
            status: entry.status(),
            fee_rate: self_rate.max(descendants_rate),
            descendants_count: aggregate.entries,
            arrival: entry.record.arrival,
            hash: entry.record.identity.raw.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum RemovalCause {
    Replacement,
    Capacity,
}

struct SelectedRemoval {
    hash: RawTxHash,
    cause: RemovalCause,
}

#[derive(Debug)]
pub(in crate::authority) struct MembershipRemoval {
    pub(in crate::authority) hash: RawTxHash,
    pub(in crate::authority) cause: RemovalCause,
    /// Continuation after removal from Accepted membership. The constructor
    /// surface admits only replacement history, so capacity eviction cannot
    /// accidentally retain an executable or uncharged owner.
    after: Option<OwnedTx>,
}

impl MembershipRemoval {
    fn terminal(hash: RawTxHash, cause: RemovalCause) -> Self {
        Self {
            hash,
            cause,
            after: None,
        }
    }

    pub(super) fn retain_replacement_history(
        &mut self,
        history: ReplacementHistoryEntry,
    ) -> Result<(), super::PlanError> {
        if self.cause != RemovalCause::Replacement || self.after.is_some() {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        self.after = Some(OwnedTx::ReplacementHistory(history));
        Ok(())
    }

    pub(super) fn terminalize(&mut self) {
        self.after = None;
    }

    pub(super) fn assign_replacement_history_identity(
        &mut self,
        version: crate::authority::state::EntryVersion,
        arrival: crate::authority::state::Arrival,
    ) -> Result<(), super::PlanError> {
        let Some(OwnedTx::ReplacementHistory(history)) = self.after.as_mut() else {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        };
        history.assign_reserved_identity(version, arrival);
        Ok(())
    }

    pub(super) fn after(&self) -> Option<&OwnedTx> {
        self.after.as_ref()
    }

    pub(super) fn take_after(&mut self) -> Option<OwnedTx> {
        self.after.take()
    }
}

pub(super) struct PreparedMembership {
    pub(super) removals: Vec<MembershipRemoval>,
    pub(super) projection: ProjectionDelta,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct CausalEdge {
    parent: RawTxHash,
    child: RawTxHash,
}

struct PreparedCausalNode {
    hash: RawTxHash,
    parents: HashSet<RawTxHash>,
    children: HashSet<RawTxHash>,
}

/// A causal edge is one semantic change even though it maintains both lookup
/// directions, so callers cannot publish `parents` without `children`.
enum CausalRelationChange {
    RemoveEdge(CausalEdge),
    InsertNode(PreparedCausalNode),
    InsertEdge(CausalEdge),
    RemoveNode(RawTxHash),
}

fn causal_change_log(
    removals: Vec<CausalEdge>,
    node_insertions: Vec<PreparedCausalNode>,
    insertions: Vec<CausalEdge>,
    node_removals: Vec<RawTxHash>,
) -> Result<Vec<CausalRelationChange>, super::PlanError> {
    let capacity = removals
        .len()
        .checked_add(node_insertions.len())
        .and_then(|count| count.checked_add(insertions.len()))
        .and_then(|count| count.checked_add(node_removals.len()))
        .ok_or(super::PlanError::Fault(
            super::AuthorityFault::CounterExhausted,
        ))?;
    let mut changes = Vec::new();
    changes
        .try_reserve(capacity)
        .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
    changes.extend(removals.into_iter().map(CausalRelationChange::RemoveEdge));
    changes.extend(
        node_insertions
            .into_iter()
            .map(CausalRelationChange::InsertNode),
    );
    changes.extend(insertions.into_iter().map(CausalRelationChange::InsertEdge));
    changes.extend(
        node_removals
            .into_iter()
            .map(CausalRelationChange::RemoveNode),
    );
    Ok(changes)
}

pub(super) struct ProjectionDelta {
    spender_changes: Vec<(OutPoint, Option<RawTxHash>)>,
    causal_changes: Vec<CausalRelationChange>,
    ancestor_changes: Vec<(RawTxHash, Option<AncestorAggregate>)>,
    aggregate_changes: Vec<(RawTxHash, Option<DescendantAggregate>)>,
    accepted_order_removals: Vec<AcceptedOrderKey>,
    accepted_order_insertions: Vec<AcceptedOrderKey>,
    eviction_removals: Vec<EvictionOrderKey>,
    eviction_insertions: Vec<EvictionOrderKey>,
    proposed_counts: super::super::shard::ShardProposedCountPlan,
    prestate: ProjectionPrestate,
    read_witness: MembershipPolicyWitness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OwnerReadFact {
    version: EntryVersion,
    kind: OwnerEntryKind,
}

struct CapturedAccepted(AcceptedEntry);

impl Deref for CapturedAccepted {
    type Target = AcceptedEntry;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct ObservedOwnerRead {
    hash: RawTxHash,
    fact: Option<OwnerReadFact>,
    vacancy_revision: Option<OwnerShardRemovalRevision>,
}

struct ObservedSpenderRead {
    out_point: OutPoint,
    spender: Option<RawTxHash>,
}

struct ObservedCausalRead {
    hash: RawTxHash,
    parents: Option<Option<HashSet<RawTxHash>>>,
    children: Option<Option<HashSet<RawTxHash>>>,
}

struct ObservedAggregateRead {
    hash: RawTxHash,
    ancestor: Option<Option<AncestorAggregate>>,
    descendant: Option<Option<DescendantAggregate>>,
}

enum MembershipPolicyWitnessMode {
    Disabled,
    Exact {
        dependency_consumer_bound: usize,
        dependency_consumer_bound_exceeded: bool,
        capacity_frontier: Option<Vec<u64>>,
    },
}

/// Demand-driven receipt for every mutable membership premise read by the
/// sole canonical evaluator. Discovery records only routed rows that policy
/// actually touches; capacity eviction is a separate explicit frontier and
/// never widens an ordinary sparse witness to all 64 shards.
pub(super) struct MembershipPolicyWitness {
    mode: MembershipPolicyWitnessMode,
    owners: Vec<ObservedOwnerRead>,
    spenders: Vec<ObservedSpenderRead>,
    dependency_consumers: Vec<ObservedDependencyConsumerRead>,
    causal: Vec<ObservedCausalRead>,
    aggregates: Vec<ObservedAggregateRead>,
    accepted_order: Vec<(AcceptedOrderKey, bool)>,
    eviction_order: Vec<(EvictionOrderKey, bool)>,
    vacant_owner_witness_complete: bool,
    #[cfg(test)]
    accepted_entry_captures: usize,
}

impl Default for MembershipPolicyWitness {
    fn default() -> Self {
        Self {
            mode: MembershipPolicyWitnessMode::Disabled,
            owners: Vec::new(),
            spenders: Vec::new(),
            dependency_consumers: Vec::new(),
            causal: Vec::new(),
            aggregates: Vec::new(),
            accepted_order: Vec::new(),
            eviction_order: Vec::new(),
            vacant_owner_witness_complete: true,
            #[cfg(test)]
            accepted_entry_captures: 0,
        }
    }
}

impl MembershipPolicyWitness {
    fn try_for_direct(
        candidate: &AcceptedEntry,
        dependency_consumer_bound: usize,
    ) -> Result<Self, super::PlanError> {
        let footprint = candidate.proof.payload().footprint();
        let outputs = candidate.record.tx.data().raw().outputs().len();
        let edge_count = footprint
            .inputs()
            .len()
            .checked_add(footprint.dependencies().len())
            .ok_or(super::PlanError::Fault(
                super::AuthorityFault::CounterExhausted,
            ))?;
        let mut witness = Self::bounded_for_shared(dependency_consumer_bound);
        witness
            .owners
            .try_reserve_exact(edge_count.checked_add(1).ok_or(super::PlanError::Fault(
                super::AuthorityFault::CounterExhausted,
            ))?)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        witness
            .spenders
            .try_reserve_exact(
                edge_count
                    .checked_add(outputs)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::CounterExhausted,
                    ))?,
            )
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        witness
            .dependency_consumers
            .try_reserve_exact(footprint.inputs().len().checked_add(outputs).ok_or(
                super::PlanError::Fault(super::AuthorityFault::CounterExhausted),
            )?)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        Ok(witness)
    }

    fn bounded_for_shared(dependency_consumer_bound: usize) -> Self {
        Self {
            mode: MembershipPolicyWitnessMode::Exact {
                dependency_consumer_bound,
                dependency_consumer_bound_exceeded: false,
                capacity_frontier: None,
            },
            ..Self::default()
        }
    }

    fn is_exact(&self) -> bool {
        matches!(self.mode, MembershipPolicyWitnessMode::Exact { .. })
    }

    fn observe_owner(
        &mut self,
        authority: &TxPoolAuthority,
        hash: &RawTxHash,
    ) -> Result<Option<OwnerReadFact>, super::PlanError> {
        if !self.is_exact() {
            return Ok(authority.entries.get(hash).as_deref().map(Self::owner_fact));
        }
        let (observed, vacancy_revision) = authority.entries.owner_fact_and_vacancy_revision(hash);
        let fact = observed.map(|(version, kind)| OwnerReadFact { version, kind });
        self.record_owner_fact(hash, fact, vacancy_revision)?;
        Ok(fact)
    }

    fn capture_owner_value(
        &mut self,
        authority: &TxPoolAuthority,
        hash: &RawTxHash,
    ) -> Result<Option<OwnedTx>, super::PlanError> {
        if !self.is_exact() {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        let (observed, vacancy_revision) = authority.entries.owner_and_vacancy_revision(hash);
        let fact = observed.as_ref().map(Self::owner_fact);
        #[cfg(test)]
        if matches!(observed, Some(OwnedTx::Accepted(_))) {
            self.accepted_entry_captures = self.accepted_entry_captures.saturating_add(1);
        }
        self.record_owner_fact(hash, fact, vacancy_revision)?;
        Ok(observed)
    }

    fn record_owner_fact(
        &mut self,
        hash: &RawTxHash,
        fact: Option<OwnerReadFact>,
        vacancy_revision: Option<OwnerShardRemovalRevision>,
    ) -> Result<(), super::PlanError> {
        if fact.is_none() && vacancy_revision.is_none() {
            self.vacant_owner_witness_complete = false;
        }
        self.owners
            .try_reserve(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        self.owners.push(ObservedOwnerRead {
            hash: hash.clone(),
            fact,
            vacancy_revision,
        });
        Ok(())
    }

    fn owner_fact(owner: &OwnedTx) -> OwnerReadFact {
        OwnerReadFact {
            version: owner.record().version,
            kind: match owner {
                OwnedTx::PreAccepted(_) => OwnerEntryKind::PreAccepted,
                OwnedTx::Accepted(_) => OwnerEntryKind::Accepted,
                OwnedTx::ReplacementHistory(_) => OwnerEntryKind::ReplacementHistory,
            },
        }
    }

    fn observe_accepted_owner(
        &mut self,
        authority: &TxPoolAuthority,
        hash: &RawTxHash,
    ) -> Result<CapturedAccepted, super::PlanError> {
        if !self.is_exact() {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        match self.capture_owner_value(authority, hash)? {
            Some(OwnedTx::Accepted(entry)) => Ok(CapturedAccepted(entry)),
            Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => Err(
                super::PlanError::Fault(super::AuthorityFault::MembershipProjection),
            ),
        }
    }

    fn observe_spender(
        &mut self,
        authority: &TxPoolAuthority,
        out_point: &OutPoint,
    ) -> Result<Option<RawTxHash>, super::PlanError> {
        let spender = authority.membership.spender(out_point);
        if !self.is_exact() {
            return Ok(spender);
        }
        self.spenders
            .try_reserve(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        self.spenders.push(ObservedSpenderRead {
            out_point: out_point.clone(),
            spender: spender.clone(),
        });
        Ok(spender)
    }

    fn observe_dependency_consumers(
        &mut self,
        authority: &TxPoolAuthority,
        key: DependencyKey,
    ) -> Result<Option<BTreeSet<RawTxHash>>, super::PlanError> {
        self.observe_dependency_consumers_inner(authority, key, true)
    }

    fn observe_dependency_consumers_inner(
        &mut self,
        authority: &TxPoolAuthority,
        key: DependencyKey,
        accepted_only: bool,
    ) -> Result<Option<BTreeSet<RawTxHash>>, super::PlanError> {
        if !self.is_exact() {
            return authority
                .dependencies
                .consumers_for(&key)
                .map_err(super::PlanError::from);
        }
        let dependency_consumer_bound = match self.mode {
            MembershipPolicyWitnessMode::Exact {
                dependency_consumer_bound,
                ..
            } => dependency_consumer_bound,
            MembershipPolicyWitnessMode::Disabled => {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
        };
        if accepted_only {
            let observed = authority
                .dependencies
                .observe_accepted_consumers_bounded_or_over_limit(key, dependency_consumer_bound);
            match observed {
                Ok(ObservedAcceptedConsumers::Within { visible, receipt }) => {
                    self.dependency_consumers.try_reserve(1).map_err(|_| {
                        super::PlanError::Backpressure(super::Backpressure::Allocation)
                    })?;
                    self.dependency_consumers.push(receipt);
                    return Ok(visible);
                }
                Ok(ObservedAcceptedConsumers::OverLimit(receipt)) => {
                    self.dependency_consumers.try_reserve(1).map_err(|_| {
                        super::PlanError::Backpressure(super::Backpressure::Allocation)
                    })?;
                    self.dependency_consumers.push(receipt);
                    if let MembershipPolicyWitnessMode::Exact {
                        dependency_consumer_bound_exceeded,
                        ..
                    } = &mut self.mode
                    {
                        *dependency_consumer_bound_exceeded = true;
                    }
                    return Err(super::PlanError::Membership(
                        MembershipReject::ComponentLimit {
                            kind: ComponentLimitKind::Mutation,
                            limit: dependency_consumer_bound,
                        },
                    ));
                }
                Err(crate::authority::dependency::DependencyError::Fanout) => {
                    return Err(super::PlanError::Stale(
                        super::StalePlan::AcceptedObservation,
                    ));
                }
                Err(error) => return Err(super::PlanError::from(error)),
            }
        }
        let observed = authority
            .dependencies
            .observe_consumers_bounded(key, dependency_consumer_bound);
        let (visible, receipt) = match observed {
            Ok(observed) => observed,
            Err(crate::authority::dependency::DependencyError::Fanout) => {
                if let MembershipPolicyWitnessMode::Exact {
                    dependency_consumer_bound_exceeded,
                    ..
                } = &mut self.mode
                {
                    *dependency_consumer_bound_exceeded = true;
                }
                return Err(super::PlanError::Stale(
                    super::StalePlan::AcceptedObservation,
                ));
            }
            Err(error) => return Err(super::PlanError::from(error)),
        };
        self.dependency_consumers
            .try_reserve(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        self.dependency_consumers.push(receipt);
        Ok(visible)
    }

    fn observe_parents(
        &mut self,
        authority: &TxPoolAuthority,
        hash: &RawTxHash,
    ) -> Result<Option<HashSet<RawTxHash>>, super::PlanError> {
        let parents = authority.membership.parents(hash);
        if self.is_exact() {
            self.record_causal(hash, Some(parents.clone()), None)?;
            #[cfg(test)]
            authority.entries.enter_membership_parent_read_probe(hash);
        }
        Ok(parents)
    }

    fn observe_children(
        &mut self,
        authority: &TxPoolAuthority,
        hash: &RawTxHash,
    ) -> Result<Option<HashSet<RawTxHash>>, super::PlanError> {
        let children = authority.membership.children(hash);
        if self.is_exact() {
            self.record_causal(hash, None, Some(children.clone()))?;
        }
        Ok(children)
    }

    fn record_causal(
        &mut self,
        hash: &RawTxHash,
        parents: Option<Option<HashSet<RawTxHash>>>,
        children: Option<Option<HashSet<RawTxHash>>>,
    ) -> Result<(), super::PlanError> {
        if !self.is_exact() {
            return Ok(());
        }
        self.causal
            .try_reserve(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        self.causal.push(ObservedCausalRead {
            hash: hash.clone(),
            parents,
            children,
        });
        Ok(())
    }

    fn observe_ancestor(
        &mut self,
        authority: &TxPoolAuthority,
        hash: &RawTxHash,
    ) -> Result<Option<AncestorAggregate>, super::PlanError> {
        let value = authority.membership.ancestor_aggregate(hash);
        if self.is_exact() {
            self.record_aggregate(hash, Some(value), None)?;
        }
        Ok(value)
    }

    fn observe_descendant(
        &mut self,
        authority: &TxPoolAuthority,
        hash: &RawTxHash,
    ) -> Result<Option<DescendantAggregate>, super::PlanError> {
        let value = authority.membership.descendant_aggregate(hash);
        if self.is_exact() {
            self.record_aggregate(hash, None, Some(value))?;
        }
        Ok(value)
    }

    fn record_aggregate(
        &mut self,
        hash: &RawTxHash,
        ancestor: Option<Option<AncestorAggregate>>,
        descendant: Option<Option<DescendantAggregate>>,
    ) -> Result<(), super::PlanError> {
        if !self.is_exact() {
            return Ok(());
        }
        self.aggregates
            .try_reserve(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        self.aggregates.push(ObservedAggregateRead {
            hash: hash.clone(),
            ancestor,
            descendant,
        });
        Ok(())
    }

    fn observe_accepted_order(
        &mut self,
        authority: &TxPoolAuthority,
        key: &AcceptedOrderKey,
    ) -> Result<bool, super::PlanError> {
        let present = authority.membership.contains_accepted_order(key);
        if !self.is_exact() {
            return Ok(present);
        }
        self.accepted_order
            .try_reserve(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        self.accepted_order.push((key.clone(), present));
        Ok(present)
    }

    fn observe_eviction_order(
        &mut self,
        authority: &TxPoolAuthority,
        key: &EvictionOrderKey,
    ) -> Result<bool, super::PlanError> {
        let present = authority.membership.contains_eviction_order(key);
        if !self.is_exact() {
            return Ok(present);
        }
        self.eviction_order
            .try_reserve(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        self.eviction_order.push((key.clone(), present));
        Ok(present)
    }

    fn observe_capacity_eviction_order(
        &mut self,
        authority: &TxPoolAuthority,
    ) -> Result<Vec<EvictionOrderKey>, super::PlanError> {
        let (order, revisions) = authority.membership.eviction_order_with_revisions()?;
        let MembershipPolicyWitnessMode::Exact {
            capacity_frontier, ..
        } = &mut self.mode
        else {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        };
        match capacity_frontier {
            Some(expected) if expected.as_slice() != revisions => Err(super::PlanError::Stale(
                super::StalePlan::AcceptedObservation,
            )),
            Some(_) => Ok(order),
            slot @ None => {
                let mut captured = Vec::new();
                captured
                    .try_reserve_exact(AUTHORITY_SHARD_COUNT)
                    .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
                captured.extend(revisions);
                *slot = Some(captured);
                Ok(order)
            }
        }
    }

    pub(in crate::authority::plan) fn is_sparse_non_capacity(&self) -> bool {
        matches!(
            self.mode,
            MembershipPolicyWitnessMode::Exact {
                capacity_frontier: None,
                ..
            }
        )
    }

    pub(in crate::authority::plan) fn has_capacity_frontier(&self) -> bool {
        matches!(
            self.mode,
            MembershipPolicyWitnessMode::Exact {
                capacity_frontier: Some(_),
                ..
            }
        )
    }

    fn sharded_read_support(&self, entries: &ShardedOwnerMap) -> ShardReadSupport {
        let mut support = ShardReadSupport::default();
        if matches!(
            self.mode,
            MembershipPolicyWitnessMode::Exact {
                capacity_frontier: Some(_),
                ..
            }
        ) {
            for shard in 0..AUTHORITY_SHARD_COUNT {
                support.insert(shard);
            }
        }
        for owner in &self.owners {
            support.insert(entries.owner_shard(&owner.hash));
        }
        for spender in &self.spenders {
            support.insert(
                entries
                    .layout
                    .router
                    .shard(b"membership/spender", &spender.out_point),
            );
        }
        for consumers in &self.dependency_consumers {
            consumers.extend_read_support(entries, &mut support);
        }
        for causal in &self.causal {
            support.insert(entries.owner_shard(&causal.hash));
        }
        for aggregate in &self.aggregates {
            support.insert(entries.owner_shard(&aggregate.hash));
        }
        for (key, _) in &self.accepted_order {
            support.insert(entries.owner_shard(key.hash()));
        }
        for (key, _) in &self.eviction_order {
            support.insert(entries.owner_shard(&key.hash));
        }
        support
    }

    fn values_are_fresh_with_dependency_stage(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        dependency_stage: Option<&StagedIngressVisibility>,
    ) -> bool {
        self.owners.iter().all(|expected| {
            let shard = entries.owner_shard(&expected.hash);
            let current = cut
                .owner(entries, &expected.hash)
                .map(|owner| OwnerReadFact {
                    version: owner.record().version,
                    kind: match owner {
                        OwnedTx::PreAccepted(_) => OwnerEntryKind::PreAccepted,
                        OwnedTx::Accepted(_) => OwnerEntryKind::Accepted,
                        OwnedTx::ReplacementHistory(_) => OwnerEntryKind::ReplacementHistory,
                    },
                });
            match expected.fact {
                Some(fact) => current == Some(fact),
                None => match expected.vacancy_revision {
                    Some(revision) => {
                        current.is_none() && cut.owner_removal_revision(shard) == revision
                    }
                    None => false,
                },
            }
        }) && self.spenders.iter().all(|expected| {
            let shard = entries
                .layout
                .router
                .shard(b"membership/spender", &expected.out_point);
            cut.projection_shard(shard)
                .spenders
                .get(&expected.out_point)
                == expected.spender.as_ref()
        }) && self.dependency_consumers.iter().all(|expected| {
            dependency_stage.map_or_else(
                || expected.is_fresh(entries, cut),
                |visibility| expected.is_fresh_before_stage(entries, cut, visibility),
            )
        }) && self.causal.iter().all(|expected| {
            let row = cut.projection_shard(entries.owner_shard(&expected.hash));
            expected
                .parents
                .as_ref()
                .is_none_or(|parents| row.parents.get(&expected.hash) == parents.as_ref())
                && expected
                    .children
                    .as_ref()
                    .is_none_or(|children| row.children.get(&expected.hash) == children.as_ref())
        }) && self.aggregates.iter().all(|expected| {
            let row = cut.projection_shard(entries.owner_shard(&expected.hash));
            expected.ancestor.is_none_or(|ancestor| {
                row.ancestor_aggregates.get(&expected.hash).copied() == ancestor
            }) && expected.descendant.is_none_or(|descendant| {
                row.descendant_aggregates.get(&expected.hash).copied() == descendant
            })
        }) && self.accepted_order.iter().all(|(key, present)| {
            cut.projection_shard(entries.owner_shard(key.hash()))
                .accepted_order
                .contains(key)
                == *present
        }) && self.eviction_order.iter().all(|(key, present)| {
            cut.projection_shard(entries.owner_shard(&key.hash))
                .eviction_order
                .contains(key)
                == *present
        }) && match &self.mode {
            MembershipPolicyWitnessMode::Exact {
                capacity_frontier: Some(expected),
                ..
            } => {
                expected.len() == AUTHORITY_SHARD_COUNT
                    && expected.iter().enumerate().all(|(shard, revision)| {
                        cut.membership_order_revision(shard).witness() == Some(*revision)
                    })
            }
            MembershipPolicyWitnessMode::Exact {
                capacity_frontier: None,
                ..
            }
            | MembershipPolicyWitnessMode::Disabled => true,
        }
    }

    fn values_are_fresh(&self, entries: &ShardedOwnerMap, cut: &ShardedOwnerWriteCut<'_>) -> bool {
        self.values_are_fresh_with_dependency_stage(entries, cut, None)
    }

    fn is_fresh(&self, entries: &ShardedOwnerMap, cut: &ShardedOwnerWriteCut<'_>) -> bool {
        !self.is_exact()
            || (self.vacant_owner_witness_complete && self.values_are_fresh(entries, cut))
    }

    fn is_fresh_before_dependency_stage(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        visibility: &StagedIngressVisibility,
    ) -> bool {
        !self.is_exact()
            || (self.vacant_owner_witness_complete
                && self.values_are_fresh_with_dependency_stage(entries, cut, Some(visibility)))
    }

    pub(in crate::authority::plan) fn bind<'authority>(
        &self,
        authority: &'authority TxPoolAuthority,
    ) -> Result<ShardedOwnerWriteCut<'authority>, super::StalePlan> {
        if !self.is_exact() {
            return Err(super::StalePlan::AcceptedObservation);
        }
        let cut = authority.entries.mixed_cut(
            self.sharded_read_support(&authority.entries),
            ShardWriteSupport::default(),
        );
        self.is_fresh(&authority.entries, &cut)
            .then_some(cut)
            .ok_or(super::StalePlan::AcceptedObservation)
    }

    fn prove_coherent(&self, authority: &TxPoolAuthority) -> bool {
        !self.is_exact() || self.bind(authority).is_ok()
    }

    #[cfg(test)]
    fn recorded_row_count(&self) -> usize {
        self.owners
            .len()
            .saturating_add(self.spenders.len())
            .saturating_add(self.dependency_consumers.len())
            .saturating_add(self.causal.len())
            .saturating_add(self.aggregates.len())
            .saturating_add(self.accepted_order.len())
            .saturating_add(self.eviction_order.len())
    }

    #[cfg(test)]
    fn accepted_entry_capture_count(&self) -> usize {
        self.accepted_entry_captures
    }

    fn expected_version(&self, hash: &RawTxHash, kind: OwnerEntryKind) -> Option<EntryVersion> {
        self.owners.iter().rev().find_map(|owner| {
            (&owner.hash == hash)
                .then_some(owner.fact)
                .flatten()
                .filter(|fact| fact.kind == kind)
                .map(|fact| fact.version)
        })
    }

    fn observed_vacant(&self, hash: &RawTxHash) -> bool {
        self.owners
            .iter()
            .rev()
            .find(|owner| &owner.hash == hash)
            .is_some_and(|owner| owner.fact.is_none() && owner.vacancy_revision.is_some())
    }
}

#[derive(Default)]
struct ProjectionPrestate {
    spenders: Vec<(OutPoint, Option<RawTxHash>)>,
    parents: Vec<(RawTxHash, Option<HashSet<RawTxHash>>)>,
    children: Vec<(RawTxHash, Option<HashSet<RawTxHash>>)>,
    ancestors: Vec<(RawTxHash, Option<AncestorAggregate>)>,
    descendants: Vec<(RawTxHash, Option<DescendantAggregate>)>,
    accepted_order: Vec<(AcceptedOrderKey, bool)>,
    eviction_order: Vec<(EvictionOrderKey, bool)>,
}

fn reserve_witness<T>(values: &mut Vec<T>, additional: usize) -> Result<(), super::PlanError> {
    values
        .try_reserve_exact(additional)
        .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))
}

impl ProjectionPrestate {
    fn capture(
        membership: &MembershipProjection,
        delta: &ProjectionDelta,
    ) -> Result<Self, super::PlanError> {
        let mut witness = Self::default();

        reserve_witness(&mut witness.spenders, delta.spender_changes.len())?;
        witness.spenders.extend(
            delta
                .spender_changes
                .iter()
                .map(|(input, _)| (input.clone(), membership.spender(input))),
        );

        let mut causal_keys = Vec::new();
        reserve_witness(
            &mut causal_keys,
            delta.causal_changes.len().saturating_mul(2),
        )?;
        for change in &delta.causal_changes {
            match change {
                CausalRelationChange::RemoveEdge(edge) | CausalRelationChange::InsertEdge(edge) => {
                    causal_keys.push(edge.parent.clone());
                    causal_keys.push(edge.child.clone());
                }
                CausalRelationChange::InsertNode(node) => causal_keys.push(node.hash.clone()),
                CausalRelationChange::RemoveNode(hash) => causal_keys.push(hash.clone()),
            }
        }
        causal_keys.sort_unstable();
        causal_keys.dedup();
        reserve_witness(&mut witness.parents, causal_keys.len())?;
        reserve_witness(&mut witness.children, causal_keys.len())?;
        for hash in causal_keys {
            witness
                .parents
                .push((hash.clone(), membership.parents(&hash)));
            witness
                .children
                .push((hash.clone(), membership.children(&hash)));
        }

        reserve_witness(&mut witness.ancestors, delta.ancestor_changes.len())?;
        witness.ancestors.extend(
            delta
                .ancestor_changes
                .iter()
                .map(|(hash, _)| (hash.clone(), membership.ancestor_aggregate(hash))),
        );
        reserve_witness(&mut witness.descendants, delta.aggregate_changes.len())?;
        witness.descendants.extend(
            delta
                .aggregate_changes
                .iter()
                .map(|(hash, _)| (hash.clone(), membership.descendant_aggregate(hash))),
        );

        let mut accepted_keys = Vec::new();
        reserve_witness(
            &mut accepted_keys,
            delta
                .accepted_order_removals
                .len()
                .saturating_add(delta.accepted_order_insertions.len()),
        )?;
        accepted_keys.extend(delta.accepted_order_removals.iter().cloned());
        accepted_keys.extend(delta.accepted_order_insertions.iter().cloned());
        accepted_keys.sort_unstable();
        accepted_keys.dedup();
        reserve_witness(&mut witness.accepted_order, accepted_keys.len())?;
        witness
            .accepted_order
            .extend(accepted_keys.into_iter().map(|key| {
                let present = membership.contains_accepted_order(&key);
                (key, present)
            }));

        let mut eviction_keys = Vec::new();
        reserve_witness(
            &mut eviction_keys,
            delta
                .eviction_removals
                .len()
                .saturating_add(delta.eviction_insertions.len()),
        )?;
        eviction_keys.extend(delta.eviction_removals.iter().cloned());
        eviction_keys.extend(delta.eviction_insertions.iter().cloned());
        eviction_keys.sort_unstable();
        eviction_keys.dedup();
        reserve_witness(&mut witness.eviction_order, eviction_keys.len())?;
        witness
            .eviction_order
            .extend(eviction_keys.into_iter().map(|key| {
                let present = membership.contains_eviction_order(&key);
                (key, present)
            }));

        Ok(witness)
    }

    fn is_fresh(&self, entries: &ShardedOwnerMap, cut: &ShardedOwnerWriteCut<'_>) -> bool {
        self.spenders.iter().all(|(input, expected)| {
            let shard = entries.layout.router.shard(b"membership/spender", input);
            cut.projection_shard(shard).spenders.get(input) == expected.as_ref()
        }) && self.parents.iter().all(|(hash, expected)| {
            cut.projection_shard(entries.owner_shard(hash))
                .parents
                .get(hash)
                == expected.as_ref()
        }) && self.children.iter().all(|(hash, expected)| {
            cut.projection_shard(entries.owner_shard(hash))
                .children
                .get(hash)
                == expected.as_ref()
        }) && self.ancestors.iter().all(|(hash, expected)| {
            cut.projection_shard(entries.owner_shard(hash))
                .ancestor_aggregates
                .get(hash)
                .copied()
                == *expected
        }) && self.descendants.iter().all(|(hash, expected)| {
            cut.projection_shard(entries.owner_shard(hash))
                .descendant_aggregates
                .get(hash)
                .copied()
                == *expected
        }) && self.accepted_order.iter().all(|(key, expected)| {
            cut.projection_shard(entries.owner_shard(key.hash()))
                .accepted_order
                .contains(key)
                == *expected
        }) && self.eviction_order.iter().all(|(key, expected)| {
            cut.projection_shard(entries.owner_shard(&key.hash))
                .eviction_order
                .contains(key)
                == *expected
        })
    }
}

impl ProjectionDelta {
    pub(super) fn empty() -> Self {
        Self {
            spender_changes: Vec::new(),
            causal_changes: Vec::new(),
            ancestor_changes: Vec::new(),
            aggregate_changes: Vec::new(),
            accepted_order_removals: Vec::new(),
            accepted_order_insertions: Vec::new(),
            eviction_removals: Vec::new(),
            eviction_insertions: Vec::new(),
            proposed_counts: super::super::shard::ShardProposedCountPlan::default(),
            prestate: ProjectionPrestate::default(),
            read_witness: MembershipPolicyWitness::default(),
        }
    }

    fn seal_prestate(
        mut self,
        membership: &MembershipProjection,
    ) -> Result<Self, super::PlanError> {
        self.prestate = ProjectionPrestate::capture(membership, &self)?;
        Ok(self)
    }

    #[cfg(test)]
    pub(in crate::authority) fn prestate_is_fresh(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.prestate.is_fresh(entries, cut) && self.read_witness.is_fresh(entries, cut)
    }

    pub(in crate::authority) fn prestate_is_fresh_before_dependency_stage(
        &self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        visibility: &StagedIngressVisibility,
    ) -> bool {
        self.prestate.is_fresh(entries, cut)
            && self
                .read_witness
                .is_fresh_before_dependency_stage(entries, cut, visibility)
    }

    pub(in crate::authority) fn sharded_read_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> ShardReadSupport {
        self.read_witness.sharded_read_support(entries)
    }

    pub(super) fn with_read_witness(mut self, read_witness: MembershipPolicyWitness) -> Self {
        self.read_witness = read_witness;
        self
    }

    pub(super) fn has_sparse_non_capacity_policy_witness(&self) -> bool {
        self.read_witness.is_sparse_non_capacity()
    }

    pub(super) fn has_capacity_frontier_policy_witness(&self) -> bool {
        self.read_witness.has_capacity_frontier()
    }

    #[cfg(test)]
    pub(in crate::authority) fn policy_witness_activity_for_foundation(&self) -> (usize, usize) {
        (
            self.read_witness.recorded_row_count(),
            self.read_witness.accepted_entry_capture_count(),
        )
    }

    pub(super) fn expected_preaccepted_version(&self, hash: &RawTxHash) -> Option<EntryVersion> {
        self.read_witness
            .expected_version(hash, OwnerEntryKind::PreAccepted)
    }

    pub(super) fn expected_accepted_version(&self, hash: &RawTxHash) -> Option<EntryVersion> {
        self.read_witness
            .expected_version(hash, OwnerEntryKind::Accepted)
    }

    pub(super) fn expected_replacement_history_version(
        &self,
        hash: &RawTxHash,
    ) -> Option<EntryVersion> {
        self.read_witness
            .expected_version(hash, OwnerEntryKind::ReplacementHistory)
    }

    pub(super) fn expected_owner_vacant(&self, hash: &RawTxHash) -> bool {
        self.read_witness.observed_vacant(hash)
    }

    pub(super) fn proposed_count_plan(&self) -> &super::super::shard::ShardProposedCountPlan {
        &self.proposed_counts
    }

    pub(super) fn take_proposed_counts(&mut self) -> super::super::shard::ShardProposedCountPlan {
        std::mem::take(&mut self.proposed_counts)
    }

    #[cfg(test)]
    pub(in crate::authority) fn erase_first_proposed_removal_for_foundation(&mut self) -> bool {
        self.proposed_counts.erase_first_removal_for_foundation()
    }

    /// Read the post-Apply spender from this change log and the authoritative
    /// pre-Apply projection. Chain dependency publication uses this instead
    /// of treating a chain-live cell as globally available while a surviving
    /// Accepted transaction still owns its pool spend.
    pub(super) fn spender_after(
        &self,
        before: &MembershipProjection,
        input: &OutPoint,
    ) -> Option<RawTxHash> {
        match self
            .spender_changes
            .binary_search_by(|(candidate, _)| candidate.cmp(input))
        {
            Ok(index) => self
                .spender_changes
                .get(index)
                .and_then(|(_, spender)| spender.clone()),
            Err(_) => before.spender(input),
        }
    }

    /// Read one projected post-Apply aggregate without materializing a second
    /// membership view. Change logs are canonical by raw hash, so effect
    /// compilation remains logarithmic in the bounded mutation component.
    pub(super) fn ancestor_after(
        &self,
        before: &MembershipProjection,
        hash: &RawTxHash,
    ) -> Option<AncestorAggregate> {
        match self
            .ancestor_changes
            .binary_search_by(|(candidate, _)| candidate.cmp(hash))
        {
            Ok(index) => self.ancestor_changes.get(index)?.1,
            Err(_) => before.ancestor_aggregate(hash),
        }
    }

    pub(super) fn descendant_after(
        &self,
        before: &MembershipProjection,
        hash: &RawTxHash,
    ) -> Option<DescendantAggregate> {
        match self
            .aggregate_changes
            .binary_search_by(|(candidate, _)| candidate.cmp(hash))
        {
            Ok(index) => self.aggregate_changes.get(index)?.1,
            Err(_) => before.descendant_aggregate(hash),
        }
    }
}

struct AggregateDelta {
    changes: Vec<(RawTxHash, Option<DescendantAggregate>)>,
    ancestor_changes: Vec<(RawTxHash, Option<AncestorAggregate>)>,
    accepted_order_removals: Vec<AcceptedOrderKey>,
    accepted_order_insertions: Vec<AcceptedOrderKey>,
    eviction_removals: Vec<EvictionOrderKey>,
    eviction_insertions: Vec<EvictionOrderKey>,
}

struct AncestorDelta {
    changes: Vec<(RawTxHash, Option<AncestorAggregate>)>,
    order_removals: Vec<AcceptedOrderKey>,
    order_insertions: Vec<AcceptedOrderKey>,
}

pub(super) struct MembershipEvaluation {
    removals: Vec<SelectedRemoval>,
    candidate_parents: HashSet<RawTxHash>,
    candidate_children: HashSet<RawTxHash>,
    aggregate: AggregateDelta,
    policy_witness: MembershipPolicyWitness,
}

/// Closed result of the sole membership policy evaluator. Rejections retain
/// the exact same demand-driven read witness as accepted evaluations, so a
/// shared effect-only terminal can revalidate the decision instead of
/// publishing a bare reason captured from a torn or stale cut.
#[expect(
    clippy::large_enum_variant,
    reason = "both Plan-only variants own already-fallible bounded witness storage; boxing would add a second allocation after canonical policy evaluation"
)]
pub(in crate::authority::plan) enum MembershipPolicyOutcome {
    Accepted(MembershipEvaluation),
    Rejected(MembershipPolicyRejection),
}

pub(in crate::authority::plan) struct MembershipPolicyRejection {
    reason: MembershipReject,
    witness: MembershipPolicyWitness,
}

impl MembershipPolicyRejection {
    pub(in crate::authority::plan) fn into_parts(
        self,
    ) -> (MembershipReject, MembershipPolicyWitness) {
        (self.reason, self.witness)
    }
}

#[expect(
    clippy::indexing_slicing,
    reason = "the sole shard router masks every domain/key result to the fixed 64-shard layout"
)]
impl MembershipProjection {
    pub(in crate::authority) fn spender(&self, input: &OutPoint) -> Option<RawTxHash> {
        self.entries.layout.shards[self.shard(b"membership/spender", input)]
            .read()
            .spenders
            .get(input)
            .cloned()
    }

    pub(in crate::authority) fn ancestor_aggregate(
        &self,
        hash: &RawTxHash,
    ) -> Option<AncestorAggregate> {
        self.entries.layout.shards[self.owner_shard(hash)]
            .read()
            .ancestor_aggregates
            .get(hash)
            .copied()
    }

    pub(in crate::authority) fn descendant_aggregate(
        &self,
        hash: &RawTxHash,
    ) -> Option<DescendantAggregate> {
        self.entries.layout.shards[self.owner_shard(hash)]
            .read()
            .descendant_aggregates
            .get(hash)
            .copied()
    }

    pub(in crate::authority) fn parents(&self, hash: &RawTxHash) -> Option<HashSet<RawTxHash>> {
        self.entries.layout.shards[self.owner_shard(hash)]
            .read()
            .parents
            .get(hash)
            .cloned()
    }

    pub(in crate::authority) fn eviction_order(&self) -> Vec<EvictionOrderKey> {
        let shards = std::array::from_fn::<_, AUTHORITY_SHARD_COUNT, _>(|shard| {
            self.entries.layout.shards[shard].read()
        });
        let count = shards.iter().map(|shard| shard.eviction_order.len()).sum();
        let mut order = Vec::with_capacity(count);
        for shard in &shards {
            order.extend(shard.eviction_order.iter().cloned());
        }
        order.sort_unstable();
        order
    }

    fn eviction_order_with_revisions(
        &self,
    ) -> Result<(Vec<EvictionOrderKey>, [u64; AUTHORITY_SHARD_COUNT]), super::PlanError> {
        // Size and version each routed partition without retaining one shard
        // guard across the next. Allocate after every guard is released, then
        // recapture each equal-revision partition into exact reserved storage.
        // The final mixed Apply cut validates all revisions simultaneously, so
        // this OCC snapshot is coherent without a population-wide planning lock.
        let mut count = 0usize;
        let mut expected_revisions = [0u64; AUTHORITY_SHARD_COUNT];
        for (revision, shard) in expected_revisions
            .iter_mut()
            .zip(self.entries.layout.shards.iter())
        {
            let shard = shard.read();
            count =
                count
                    .checked_add(shard.eviction_order.len())
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::CounterExhausted,
                    ))?;
            *revision =
                shard
                    .membership_order_revision
                    .witness()
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::CounterExhausted,
                    ))?;
        }
        let mut order = Vec::new();
        order
            .try_reserve_exact(count)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        for (shard, expected_revision) in self.entries.layout.shards.iter().zip(expected_revisions)
        {
            let shard = shard.read();
            if shard.membership_order_revision.witness() != Some(expected_revision) {
                return Err(super::PlanError::Stale(
                    super::StalePlan::AcceptedObservation,
                ));
            }
            order.extend(shard.eviction_order.iter().cloned());
        }
        order.sort_unstable();
        Ok((order, expected_revisions))
    }

    pub(in crate::authority) fn contains_accepted_order(&self, key: &AcceptedOrderKey) -> bool {
        self.entries.layout.shards[self.owner_shard(key.hash())]
            .read()
            .accepted_order
            .contains(key)
    }

    pub(in crate::authority) fn contains_eviction_order(&self, key: &EvictionOrderKey) -> bool {
        self.entries.layout.shards[self.owner_shard(&key.hash)]
            .read()
            .eviction_order
            .contains(key)
    }

    fn contains_parent_node(&self, hash: &RawTxHash) -> bool {
        self.parents(hash).is_some()
    }

    fn contains_child_node(&self, hash: &RawTxHash) -> bool {
        self.children(hash).is_some()
    }

    pub(super) fn children(&self, hash: &RawTxHash) -> Option<HashSet<RawTxHash>> {
        self.entries.layout.shards[self.owner_shard(hash)]
            .read()
            .children
            .get(hash)
            .cloned()
    }

    /// Read one Accepted owner and its co-located child row under the same
    /// physical shard guard. An owner that disappeared or changed phase is
    /// legal optimistic progress; an Accepted owner without its mandatory
    /// row is a stable projection contradiction.
    fn accepted_child_row(&self, hash: &RawTxHash) -> Result<HashSet<RawTxHash>, super::PlanError> {
        let (accepted, children) = self.entries.accepted_child_row_observation(hash);
        if !accepted {
            return Err(super::PlanError::Stale(
                super::StalePlan::AcceptedObservation,
            ));
        }
        children.ok_or(super::PlanError::Fault(
            super::AuthorityFault::MembershipProjection,
        ))
    }

    pub(super) fn reserve_owner_insertion_capacity<'input, 'owner>(
        &self,
        inputs: impl IntoIterator<Item = &'input OutPoint>,
        owners: impl IntoIterator<Item = &'owner RawTxHash>,
    ) -> Result<(), super::PlanError> {
        let mut input_additions = [0usize; AUTHORITY_SHARD_COUNT];
        for input in inputs {
            let shard = self.shard(b"membership/spender", input);
            input_additions[shard] =
                input_additions[shard]
                    .checked_add(1)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::CounterExhausted,
                    ))?;
        }
        let mut owner_additions = [0usize; AUTHORITY_SHARD_COUNT];
        for owner in owners {
            let shard = self.owner_shard(owner);
            owner_additions[shard] =
                owner_additions[shard]
                    .checked_add(1)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::CounterExhausted,
                    ))?;
        }
        for (shard, (inputs, owners)) in self
            .entries
            .layout
            .shards
            .iter()
            .zip(input_additions.into_iter().zip(owner_additions))
        {
            if inputs == 0 && owners == 0 {
                continue;
            }
            let mut shard = shard.write();
            shard
                .spenders
                .try_reserve(inputs)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            shard
                .parents
                .try_reserve(owners)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            shard
                .children
                .try_reserve(owners)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            shard
                .ancestor_aggregates
                .try_reserve(owners)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            shard
                .descendant_aggregates
                .try_reserve(owners)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        }
        Ok(())
    }

    pub(super) fn reserve_child_row(
        &self,
        parent: &RawTxHash,
        additional: usize,
    ) -> Result<(), super::PlanError> {
        self.entries.layout.shards[self.owner_shard(parent)]
            .write()
            .children
            .get_mut(parent)
            .ok_or(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ))?
            .try_reserve(additional)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))
    }

    pub(super) fn reserve_parent_row(
        &self,
        child: &RawTxHash,
        additional: usize,
    ) -> Result<(), super::PlanError> {
        self.entries.layout.shards[self.owner_shard(child)]
            .write()
            .parents
            .get_mut(child)
            .ok_or(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ))?
            .try_reserve(additional)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))
    }

    pub(super) fn apply(&self, delta: ProjectionDelta) {
        let support = delta.sharded_write_support(&self.entries);
        let mut cut = self.entries.write_cut(support);
        delta.apply_sharded(&self.entries, &mut cut);
    }
}

impl ProjectionDelta {
    pub(in crate::authority) fn sharded_write_support(
        &self,
        entries: &ShardedOwnerMap,
    ) -> super::super::shard::ShardWriteSupport {
        let mut support = super::super::shard::ShardWriteSupport::default();
        for (input, _) in &self.spender_changes {
            support.insert(entries.layout.router.shard(b"membership/spender", input));
        }
        for change in &self.causal_changes {
            match change {
                CausalRelationChange::RemoveEdge(edge) | CausalRelationChange::InsertEdge(edge) => {
                    support.insert(entries.owner_shard(&edge.parent));
                    support.insert(entries.owner_shard(&edge.child));
                }
                CausalRelationChange::InsertNode(node) => {
                    support.insert(entries.owner_shard(&node.hash));
                }
                CausalRelationChange::RemoveNode(hash) => {
                    support.insert(entries.owner_shard(hash));
                }
            }
        }
        for (hash, _) in &self.ancestor_changes {
            support.insert(entries.owner_shard(hash));
        }
        for (hash, _) in &self.aggregate_changes {
            support.insert(entries.owner_shard(hash));
        }
        for key in self
            .accepted_order_removals
            .iter()
            .chain(&self.accepted_order_insertions)
        {
            support.insert(entries.owner_shard(key.hash()));
        }
        for key in self
            .eviction_removals
            .iter()
            .chain(&self.eviction_insertions)
        {
            support.insert(entries.owner_shard(&key.hash));
        }
        support
    }

    pub(in crate::authority) fn apply_sharded(
        self,
        entries: &ShardedOwnerMap,
        cut: &mut ShardedOwnerWriteCut<'_>,
    ) {
        let mut order_changed = [false; AUTHORITY_SHARD_COUNT];
        for key in self
            .accepted_order_removals
            .iter()
            .chain(&self.accepted_order_insertions)
        {
            if let Some(changed) = order_changed.get_mut(entries.owner_shard(key.hash())) {
                *changed = true;
            }
        }
        for key in self
            .eviction_removals
            .iter()
            .chain(&self.eviction_insertions)
        {
            if let Some(changed) = order_changed.get_mut(entries.owner_shard(&key.hash)) {
                *changed = true;
            }
        }
        for (input, spender) in self.spender_changes {
            let shard = entries.layout.router.shard(b"membership/spender", &input);
            let spenders = &mut cut.projection_shard_mut(shard).spenders;
            match spender {
                Some(spender) => {
                    spenders.insert(input, spender);
                }
                None => {
                    spenders.remove(&input);
                }
            }
        }
        for change in self.causal_changes {
            match change {
                CausalRelationChange::RemoveEdge(edge) => {
                    let children_shard = entries.owner_shard(&edge.parent);
                    if let Some(children) = cut
                        .projection_shard_mut(children_shard)
                        .children
                        .get_mut(&edge.parent)
                    {
                        children.remove(&edge.child);
                    }
                    let parents_shard = entries.owner_shard(&edge.child);
                    if let Some(parents) = cut
                        .projection_shard_mut(parents_shard)
                        .parents
                        .get_mut(&edge.child)
                    {
                        parents.remove(&edge.parent);
                    }
                }
                CausalRelationChange::InsertNode(node) => {
                    let parents_shard = entries.owner_shard(&node.hash);
                    cut.projection_shard_mut(parents_shard)
                        .parents
                        .insert(node.hash.clone(), node.parents);
                    let children_shard = entries.owner_shard(&node.hash);
                    cut.projection_shard_mut(children_shard)
                        .children
                        .insert(node.hash, node.children);
                }
                CausalRelationChange::InsertEdge(edge) => {
                    let children_shard = entries.owner_shard(&edge.parent);
                    if let Some(children) = cut
                        .projection_shard_mut(children_shard)
                        .children
                        .get_mut(&edge.parent)
                    {
                        children.insert(edge.child.clone());
                    }
                    let parents_shard = entries.owner_shard(&edge.child);
                    if let Some(parents) = cut
                        .projection_shard_mut(parents_shard)
                        .parents
                        .get_mut(&edge.child)
                    {
                        parents.insert(edge.parent);
                    }
                }
                CausalRelationChange::RemoveNode(hash) => {
                    let parents_shard = entries.owner_shard(&hash);
                    cut.projection_shard_mut(parents_shard)
                        .parents
                        .remove(&hash);
                    let children_shard = entries.owner_shard(&hash);
                    cut.projection_shard_mut(children_shard)
                        .children
                        .remove(&hash);
                }
            }
        }
        for (hash, aggregate) in self.ancestor_changes {
            let shard = entries.owner_shard(&hash);
            let rows = &mut cut.projection_shard_mut(shard).ancestor_aggregates;
            match aggregate {
                Some(aggregate) => {
                    rows.insert(hash, aggregate);
                }
                None => {
                    rows.remove(&hash);
                }
            }
        }
        for (hash, aggregate) in self.aggregate_changes {
            let shard = entries.owner_shard(&hash);
            let rows = &mut cut.projection_shard_mut(shard).descendant_aggregates;
            match aggregate {
                Some(aggregate) => {
                    rows.insert(hash, aggregate);
                }
                None => {
                    rows.remove(&hash);
                }
            }
        }
        for key in self.accepted_order_removals {
            let shard = entries.owner_shard(key.hash());
            cut.projection_shard_mut(shard).accepted_order.remove(&key);
        }
        for key in self.accepted_order_insertions {
            let shard = entries.owner_shard(key.hash());
            cut.projection_shard_mut(shard).accepted_order.insert(key);
        }
        for key in self.eviction_removals {
            let shard = entries.owner_shard(&key.hash);
            cut.projection_shard_mut(shard).eviction_order.remove(&key);
        }
        for key in self.eviction_insertions {
            let shard = entries.owner_shard(&key.hash);
            cut.projection_shard_mut(shard).eviction_order.insert(key);
        }
        for (shard, changed) in order_changed.into_iter().enumerate() {
            if changed {
                cut.projection_shard_mut(shard)
                    .membership_order_revision
                    .advance();
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/support/plan_membership.rs"]
pub(in crate::authority) mod test_support;

impl TxPoolAuthority {
    /// Shared administrative planning additionally retains every traversed
    /// child row. The immutable Accepted-entry ceiling is the traversal
    /// bound: a live owner count is only coherent under the retired outer
    /// writer and cannot bound an optimistic shared Plan.
    pub(super) fn administrative_descendant_closure_witness(
        &self,
        root: &RawTxHash,
    ) -> Result<AdministrativeDescendantClosure, super::PlanError> {
        if !matches!(
            self.entries.get(root).as_deref(),
            Some(OwnedTx::Accepted(_))
        ) {
            return Err(super::PlanError::Stale(super::StalePlan::Phase));
        }
        let mut reader = PolicyContext::exclusive(self);
        let closure = Self::bounded_descendant_postorder_with_reader(
            std::slice::from_ref(root),
            &HashSet::new(),
            self.resources_for_plan().limits().accepted_entry_limit(),
            ComponentLimitKind::Mutation,
            &mut reader,
        );
        match closure {
            // `remaining_limit` is the immutable Accepted population ceiling,
            // so a real closure cannot reach it. Treat such a result as a
            // projection fault instead of a public mutation rule.
            Err(super::PlanError::Membership(_)) => Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            )),
            Ok(closure) => Ok(AdministrativeDescendantClosure {
                ordered: closure.ordered,
                child_rows: closure.child_rows,
            }),
            Err(error) => Err(error),
        }
    }

    pub(super) fn evaluate_preaccepted_membership_policy(
        &self,
        hash: &RawTxHash,
        before: &PreAcceptedEntry,
        candidate: &AcceptedEntry,
        bounded_dependency_consumers: bool,
    ) -> Result<MembershipPolicyOutcome, super::PlanError> {
        Self::validate_preaccepted_membership_subject(hash, before, candidate)?;
        self.evaluate_membership_policy_with_dependency_bound(
            hash,
            candidate,
            bounded_dependency_consumers.then_some(self.membership_config.max_component),
        )
    }

    pub(super) fn prepare_membership_after_evaluation(
        &self,
        hash: &RawTxHash,
        before: &PreAcceptedEntry,
        candidate: &AcceptedEntry,
        evaluation: MembershipEvaluation,
    ) -> Result<PreparedMembership, super::PlanError> {
        Self::validate_preaccepted_membership_subject(hash, before, candidate)?;
        self.compile_membership_evaluation(hash, candidate, evaluation)
    }

    fn validate_preaccepted_membership_subject(
        hash: &RawTxHash,
        before: &PreAcceptedEntry,
        candidate: &AcceptedEntry,
    ) -> Result<(), super::PlanError> {
        if before.record.identity.raw != *hash
            || candidate.record.identity != before.record.identity
            || candidate.provenance != before.source.accepted_provenance()
            || candidate.record.arrival != before.record.arrival
        {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        Ok(())
    }

    /// Compile the already-adjudicated canonical Direct policy result into
    /// the sole membership projection delta.
    pub(super) fn prepare_direct_membership_after_evaluation(
        &self,
        hash: &RawTxHash,
        candidate: &AcceptedEntry,
        evaluation: MembershipEvaluation,
    ) -> Result<PreparedMembership, super::PlanError> {
        self.compile_membership_evaluation(hash, candidate, evaluation)
    }

    /// Compile the established feature-internal `PlugEntry` hook without
    /// granting it replacement or eviction authority. The ordinary policy
    /// evaluator still proves all graph and capacity constraints; a result
    /// that needs to remove any resident owner is returned as a typed
    /// non-displacing outcome before a mutation delta exists.
    #[cfg(any(test, feature = "internal"))]
    pub(super) fn prepare_non_displacing_internal_candidate(
        &mut self,
        hash: &RawTxHash,
        candidate: &AcceptedEntry,
    ) -> Result<Option<PreparedMembership>, super::PlanError> {
        let evaluation = self.evaluate_membership_candidate(hash, candidate)?;
        if !evaluation.removals.is_empty() {
            return Ok(None);
        }
        self.compile_membership_evaluation(hash, candidate, evaluation)
            .map(Some)
    }

    /// Evaluate RBF, graph, and capacity policy without constructing an
    /// authoritative mutation. Local Plan and TestAccept share this exact
    /// read-only decision; only Local compiles the result into projection and
    /// ownership deltas.
    pub(super) fn evaluate_membership_candidate(
        &self,
        hash: &RawTxHash,
        candidate: &AcceptedEntry,
    ) -> Result<MembershipEvaluation, super::PlanError> {
        match self.evaluate_membership_policy(hash, candidate)? {
            MembershipPolicyOutcome::Accepted(evaluation) => Ok(evaluation),
            MembershipPolicyOutcome::Rejected(rejection) => {
                let (reason, _witness) = rejection.into_parts();
                Err(super::PlanError::Membership(reason))
            }
        }
    }

    pub(in crate::authority::plan) fn evaluate_membership_policy(
        &self,
        hash: &RawTxHash,
        candidate: &AcceptedEntry,
    ) -> Result<MembershipPolicyOutcome, super::PlanError> {
        self.evaluate_membership_policy_with_dependency_bound(hash, candidate, None)
    }

    fn evaluate_membership_policy_with_dependency_bound(
        &self,
        hash: &RawTxHash,
        candidate: &AcceptedEntry,
        dependency_consumer_bound: Option<usize>,
    ) -> Result<MembershipPolicyOutcome, super::PlanError> {
        match dependency_consumer_bound {
            Some(bound) => Self::evaluate_membership_policy_with_reader(
                hash,
                candidate,
                PolicyContext::optimistic(self, bound),
            ),
            None => Self::evaluate_membership_policy_with_reader(
                hash,
                candidate,
                PolicyContext::exclusive(self),
            ),
        }
    }

    fn evaluate_membership_policy_with_reader<Mode>(
        hash: &RawTxHash,
        candidate: &AcceptedEntry,
        mut reader: PolicyContext<'_, Mode>,
    ) -> Result<MembershipPolicyOutcome, super::PlanError>
    where
        Mode: PolicyMode,
    {
        reader.observe_owner(hash)?;
        let evaluated = (|| {
            let mandatory = rbf::replacement_removals(candidate, &mut reader)?;
            eviction::complete_removals(hash, candidate, mandatory, &mut reader)
        })();
        let policy_witness = reader.finish()?;
        match evaluated {
            Ok(mut evaluation) => {
                evaluation.policy_witness = policy_witness;
                Ok(MembershipPolicyOutcome::Accepted(evaluation))
            }
            Err(super::PlanError::Membership(reason)) => Ok(MembershipPolicyOutcome::Rejected(
                MembershipPolicyRejection {
                    reason,
                    witness: policy_witness,
                },
            )),
            Err(error) => Err(error),
        }
    }

    /// Preallocate one Direct policy witness from immutable candidate shape,
    /// then capture the complete current owner value into that same witness.
    /// The canonical evaluator receives this witness and records the owner
    /// again, so an interposed owner change makes the final exact cut stale
    /// instead of splicing provenance, arrival and policy from two states.
    pub(super) fn capture_direct_membership_subject(
        &self,
        hash: &RawTxHash,
        sizing_candidate: &AcceptedEntry,
    ) -> Result<(Option<OwnedTx>, MembershipPolicyWitness), super::PlanError> {
        let mut witness = MembershipPolicyWitness::try_for_direct(
            sizing_candidate,
            self.membership_config.max_component,
        )?;
        let existing = witness.capture_owner_value(self, hash)?;
        if !witness.prove_coherent(self) {
            return Err(super::PlanError::Stale(
                super::StalePlan::AcceptedObservation,
            ));
        }
        Ok((existing, witness))
    }

    pub(super) fn evaluate_direct_membership_policy(
        &self,
        hash: &RawTxHash,
        candidate: &AcceptedEntry,
        witness: MembershipPolicyWitness,
    ) -> Result<MembershipPolicyOutcome, super::PlanError> {
        Self::evaluate_membership_policy_with_reader(
            hash,
            candidate,
            PolicyContext::optimistic_with_witness(self, witness),
        )
    }
    #[cfg(test)]
    pub(in crate::authority) fn direct_absent_matches_canonical_for_foundation(
        &self,
        hash: &RawTxHash,
        candidate: &AcceptedEntry,
    ) -> Result<bool, super::PlanError> {
        let canonical = self.evaluate_membership_candidate(hash, candidate)?;
        let expected = eviction::pure_leaf_evaluation(hash, candidate)?;
        Ok(canonical.removals.is_empty()
            && canonical.candidate_parents.is_empty()
            && canonical.candidate_children.is_empty()
            && canonical.aggregate.changes == expected.aggregate.changes
            && canonical.aggregate.ancestor_changes == expected.aggregate.ancestor_changes
            && canonical.aggregate.accepted_order_removals.is_empty()
            && canonical.aggregate.accepted_order_insertions
                == expected.aggregate.accepted_order_insertions
            && canonical.aggregate.eviction_removals.is_empty()
            && canonical.aggregate.eviction_insertions == expected.aggregate.eviction_insertions)
    }
    fn compile_membership_evaluation(
        &self,
        hash: &RawTxHash,
        candidate: &AcceptedEntry,
        evaluation: MembershipEvaluation,
    ) -> Result<PreparedMembership, super::PlanError> {
        let MembershipEvaluation {
            removals: selected_removals,
            candidate_parents,
            candidate_children,
            aggregate,
            policy_witness,
        } = evaluation;
        let projection = self.prepare_projection_change(
            hash,
            candidate,
            &selected_removals,
            candidate_parents,
            candidate_children,
            aggregate,
        )?;
        let mut removals = Vec::new();
        removals
            .try_reserve(selected_removals.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        removals.extend(
            selected_removals
                .into_iter()
                .map(|selected| MembershipRemoval::terminal(selected.hash, selected.cause)),
        );
        Ok(PreparedMembership {
            removals,
            projection: projection.with_read_witness(policy_witness),
        })
    }

    /// Compile the accepted-membership part of one chain transition from its
    /// projected final owner set. Unlike RBF removal, committed parents may
    /// have surviving children, so every incident causal edge is removed and
    /// descendant aggregates subtract each removed entry through removed
    /// intermediate ancestors.
    pub(super) fn prepare_chain_projection(
        &self,
        removals: &AcceptedRemovalSet,
        status_changes: &HashMap<RawTxHash, AcceptedEntry>,
    ) -> Result<ProjectionDelta, super::PlanError> {
        if status_changes.keys().any(|hash| removals.contains(hash)) {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        let mut removed = HashSet::new();
        removed
            .try_reserve(removals.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        removed.extend(removals.iter().cloned());
        if removed.len() != removals.len() {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        let ancestor = self.prepare_chain_ancestor_delta(removals, &removed)?;

        let mut status_count_changes = Vec::new();
        status_count_changes
            .try_reserve(removals.len().checked_add(status_changes.len()).ok_or(
                super::PlanError::Fault(super::AuthorityFault::CounterExhausted),
            )?)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut relation_capacity = 0usize;
        let mut input_capacity = 0usize;
        for hash in removals.iter() {
            let entry = self.accepted_entry(hash)?;
            status_count_changes.push((hash.clone(), Some(entry.status()), None));
            input_capacity = input_capacity
                .checked_add(entry.proof.payload().footprint.inputs().len())
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
            let parents = self
                .membership
                .parents(hash)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            let children = self
                .membership
                .children(hash)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            relation_capacity = relation_capacity
                .checked_add(parents.len())
                .and_then(|count| count.checked_add(children.len()))
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
        }
        for (hash, after) in status_changes {
            let before = self.accepted_entry(hash)?;
            if before.record.identity != after.record.identity
                || before.proof != after.proof
                || before.proposal == after.proposal
            {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            status_count_changes.push((hash.clone(), Some(before.status()), Some(after.status())));
        }
        let proposed_counts = self.entries.plan_proposed_counts(
            status_count_changes
                .iter()
                .map(|(hash, before, after)| (hash, *before, *after)),
        )?;

        let mut spender_changes = Vec::new();
        spender_changes
            .try_reserve(input_capacity)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut causal_removals = Vec::new();
        causal_removals
            .try_reserve(relation_capacity)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let aggregate_capacity = removals
            .len()
            .checked_mul(self.membership_config.max_ancestors)
            .map_or(self.membership_config.max_component, |capacity| {
                capacity.min(self.membership_config.max_component)
            });
        let mut projected_aggregates = HashMap::new();
        projected_aggregates
            .try_reserve(aggregate_capacity)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;

        for hash in removals.iter() {
            let entry = self.accepted_entry(hash)?;
            for input in entry.proof.payload().footprint.inputs() {
                if self.membership.spender(input) != Some(hash.clone()) {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                spender_changes.push((input.clone(), None));
            }
            let parents = self
                .membership
                .parents(hash)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            let children = self
                .membership
                .children(hash)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            for parent in parents {
                if !self
                    .membership
                    .children(&parent)
                    .is_some_and(|children| children.contains(hash))
                {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                causal_removals.push(CausalEdge {
                    parent: parent.clone(),
                    child: hash.clone(),
                });
            }
            for child in children {
                if !self
                    .membership
                    .parents(&child)
                    .is_some_and(|parents| parents.contains(hash))
                {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                causal_removals.push(CausalEdge {
                    parent: hash.clone(),
                    child: child.clone(),
                });
            }

            for ancestor in self.collect_surviving_ancestors_through_removals(hash, &removed)? {
                let current = projected_aggregates
                    .get(&ancestor)
                    .copied()
                    .or_else(|| self.membership.descendant_aggregate(&ancestor));
                let next = current
                    .and_then(|aggregate| aggregate.checked_sub_entry(&entry))
                    .filter(|aggregate| aggregate.entries != 0)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
                if !projected_aggregates.contains_key(&ancestor)
                    && projected_aggregates.len() >= self.membership_config.max_component
                {
                    return Err(super::PlanError::Backpressure(
                        super::Backpressure::GenerationReplacement,
                    ));
                }
                projected_aggregates.insert(ancestor, next);
            }
        }

        spender_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        causal_removals.sort_unstable();
        causal_removals.dedup();
        let mut causal_node_removals = Vec::new();
        causal_node_removals
            .try_reserve(removals.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        causal_node_removals.extend(removals.iter().cloned());
        let causal_changes = causal_change_log(
            causal_removals,
            Vec::new(),
            Vec::new(),
            causal_node_removals,
        )?;

        let eviction_capacity = removals
            .len()
            .checked_add(projected_aggregates.len())
            .and_then(|count| count.checked_add(status_changes.len()))
            .ok_or(super::PlanError::Fault(
                super::AuthorityFault::CounterExhausted,
            ))?;
        let mut eviction_removals = Vec::new();
        let mut eviction_insertions = Vec::new();
        eviction_removals
            .try_reserve(eviction_capacity)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        eviction_insertions
            .try_reserve(
                projected_aggregates
                    .len()
                    .checked_add(status_changes.len())
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::CounterExhausted,
                    ))?,
            )
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        for hash in removals.iter() {
            let entry = self.accepted_entry(hash)?;
            let aggregate =
                self.membership
                    .descendant_aggregate(hash)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
            let key = EvictionOrderKey::new(&entry, aggregate);
            if !self.membership.contains_eviction_order(&key) {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            eviction_removals.push(key);
        }

        let mut aggregate_changes = Vec::new();
        aggregate_changes
            .try_reserve(
                removals
                    .len()
                    .checked_add(projected_aggregates.len())
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::CounterExhausted,
                    ))?,
            )
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        aggregate_changes.extend(removals.iter().cloned().map(|hash| (hash, None)));
        for (hash, after) in status_changes {
            if projected_aggregates.contains_key(hash) {
                continue;
            }
            let before = self.accepted_entry(hash)?;
            let aggregate =
                self.membership
                    .descendant_aggregate(hash)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
            let before_key = EvictionOrderKey::new(&before, aggregate);
            if !self.membership.contains_eviction_order(&before_key) {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            eviction_removals.push(before_key);
            eviction_insertions.push(EvictionOrderKey::new(after, aggregate));
        }
        let mut ordered_aggregates = Vec::new();
        ordered_aggregates
            .try_reserve(projected_aggregates.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        ordered_aggregates.extend(projected_aggregates);
        ordered_aggregates.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        for (hash, after_aggregate) in ordered_aggregates {
            let before = self.accepted_entry(&hash)?;
            let before_aggregate =
                self.membership
                    .descendant_aggregate(&hash)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
            let before_key = EvictionOrderKey::new(&before, before_aggregate);
            if !self.membership.contains_eviction_order(&before_key) {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            eviction_removals.push(before_key);
            let insertion = status_changes.get(&hash).map_or_else(
                || EvictionOrderKey::new(&before, after_aggregate),
                |after| EvictionOrderKey::new(after, after_aggregate),
            );
            eviction_insertions.push(insertion);
            aggregate_changes.push((hash, Some(after_aggregate)));
        }
        eviction_removals.sort_unstable();
        eviction_removals.dedup();
        eviction_insertions.sort_unstable();
        eviction_insertions.dedup();
        aggregate_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));

        ProjectionDelta {
            spender_changes,
            causal_changes,
            ancestor_changes: ancestor.changes,
            aggregate_changes,
            accepted_order_removals: ancestor.order_removals,
            accepted_order_insertions: ancestor.order_insertions,
            eviction_removals,
            eviction_insertions,
            proposed_counts,
            prestate: Default::default(),
            read_witness: MembershipPolicyWitness::default(),
        }
        .seal_prestate(&self.membership)
    }

    /// Recompute only accepted descendants whose package score can change
    /// when accepted ancestors leave on a chain cut. This work is inherent in
    /// exact CPFP ordering: every affected survivor can acquire a different
    /// winner relation. It runs in Plan, and Apply receives only replacement
    /// aggregates and ordered keys.
    fn prepare_chain_ancestor_delta(
        &self,
        removals: &AcceptedRemovalSet,
        removed: &HashSet<RawTxHash>,
    ) -> Result<AncestorDelta, super::PlanError> {
        let mut visited = HashSet::new();
        visited
            .try_reserve(removals.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut affected = HashSet::new();
        let mut frontier = VecDeque::new();
        frontier
            .try_reserve(removals.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        frontier.extend(removals.iter().cloned());
        while let Some(hash) = frontier.pop_front() {
            if visited.contains(&hash) {
                continue;
            }
            visited
                .try_reserve(1)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            visited.insert(hash.clone());
            let children = self
                .membership
                .children(&hash)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            frontier
                .try_reserve(children.len())
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            for child in children {
                frontier.push_back(child.clone());
                if !removals.contains(&child) && !affected.contains(&child) {
                    affected.try_reserve(1).map_err(|_| {
                        super::PlanError::Backpressure(super::Backpressure::Allocation)
                    })?;
                    affected.insert(child.clone());
                }
            }
        }

        let change_capacity =
            removals
                .len()
                .checked_add(affected.len())
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
        let mut changes = Vec::new();
        changes
            .try_reserve(change_capacity)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut order_removals = Vec::new();
        order_removals
            .try_reserve(change_capacity)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut order_insertions = Vec::new();
        order_insertions
            .try_reserve(affected.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;

        for hash in removals.iter() {
            let entry = self.accepted_entry(hash)?;
            let aggregate =
                self.membership
                    .ancestor_aggregate(hash)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
            let key = AcceptedOrderKey::new(&entry, aggregate);
            if !self.membership.contains_accepted_order(&key) {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            changes.push((hash.clone(), None));
            order_removals.push(key);
        }

        let mut ordered_affected = Vec::new();
        ordered_affected
            .try_reserve(affected.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        ordered_affected.extend(affected);
        ordered_affected.sort_unstable();
        for hash in ordered_affected {
            let entry = self.accepted_entry(&hash)?;
            let before =
                self.membership
                    .ancestor_aggregate(&hash)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
            let before_key = AcceptedOrderKey::new(&entry, before);
            if !self.membership.contains_accepted_order(&before_key) {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            let parents = self
                .membership
                .parents(&hash)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            let ancestors = self.collect_surviving_ancestors(&parents, removed)?;
            let mut after = AncestorAggregate::one(&entry);
            for ancestor in ancestors {
                after = after
                    .checked_add_entry(&*self.accepted_entry(&ancestor)?)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
            }
            changes.push((hash.clone(), Some(after)));
            order_removals.push(before_key);
            order_insertions.push(AcceptedOrderKey::new(&entry, after));
        }
        order_removals.sort_unstable();
        order_insertions.sort_unstable();
        Ok(AncestorDelta {
            changes,
            order_removals,
            order_insertions,
        })
    }

    fn prepare_projection_change(
        &self,
        hash: &RawTxHash,
        candidate: &AcceptedEntry,
        removals: &[SelectedRemoval],
        parents: HashSet<RawTxHash>,
        children: HashSet<RawTxHash>,
        aggregate: AggregateDelta,
    ) -> Result<ProjectionDelta, super::PlanError> {
        let mut removed = HashSet::new();
        removed
            .try_reserve(removals.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        removed.extend(removals.iter().map(|removal| removal.hash.clone()));
        let mut status_count_changes = Vec::new();
        status_count_changes
            .try_reserve(
                removals
                    .len()
                    .checked_add(1)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::CounterExhausted,
                    ))?,
            )
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        for planned in removals {
            let removal = &planned.hash;
            let entry = self.accepted_entry(removal)?;
            status_count_changes.push((removal.clone(), Some(entry.status()), None));
        }
        status_count_changes.push((hash.clone(), None, Some(candidate.status())));
        let proposed_counts = self.entries.plan_proposed_counts(
            status_count_changes
                .iter()
                .map(|(hash, before, after)| (hash, *before, *after)),
        )?;

        let footprint = &candidate.proof.payload().footprint;
        let mut removal_inputs = 0usize;
        let mut removal_causal_edges = 0usize;
        for planned in removals {
            let entry = self.accepted_entry(&planned.hash)?;
            removal_inputs = removal_inputs
                .checked_add(entry.proof.payload().footprint.inputs().len())
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
            let removal_parents =
                self.membership
                    .parents(&planned.hash)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
            let removal_children =
                self.membership
                    .children(&planned.hash)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
            removal_causal_edges = removal_causal_edges
                .checked_add(removal_parents.len())
                .and_then(|count| count.checked_add(removal_children.len()))
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
        }
        let spender_capacity =
            removal_inputs
                .checked_add(footprint.inputs().len())
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
        let mut spender_changes = Vec::new();
        spender_changes
            .try_reserve(spender_capacity)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut causal_edge_removals = Vec::new();
        causal_edge_removals
            .try_reserve(removal_causal_edges)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let causal_insertion_capacity =
            parents
                .len()
                .checked_add(children.len())
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
        let mut causal_edge_insertions = Vec::new();
        causal_edge_insertions
            .try_reserve(causal_insertion_capacity)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;

        for planned in removals {
            let removal = &planned.hash;
            let entry = self.accepted_entry(removal)?;
            for input in entry.proof.payload().footprint.inputs() {
                if self.membership.spender(input) != Some(removal.clone()) {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                spender_changes.push((input.clone(), None));
            }
            let removal_parents =
                self.membership
                    .parents(removal)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
            let removal_children =
                self.membership
                    .children(removal)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
            if removal_children
                .iter()
                .any(|child| !removed.contains(child))
            {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            for parent in removal_parents {
                let parent_children =
                    self.membership
                        .children(&parent)
                        .ok_or(super::PlanError::Fault(
                            super::AuthorityFault::MembershipProjection,
                        ))?;
                if !parent_children.contains(removal) {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                causal_edge_removals.push(CausalEdge {
                    parent: parent.clone(),
                    child: removal.clone(),
                });
            }
            for child in removal_children {
                let child_parents =
                    self.membership
                        .parents(&child)
                        .ok_or(super::PlanError::Fault(
                            super::AuthorityFault::MembershipProjection,
                        ))?;
                if !child_parents.contains(removal) {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                causal_edge_removals.push(CausalEdge {
                    parent: removal.clone(),
                    child: child.clone(),
                });
            }
        }

        for input in footprint.inputs() {
            if self
                .membership
                .spender(input)
                .is_some_and(|spender| !removed.contains(&spender))
            {
                return Err(super::PlanError::Membership(
                    MembershipReject::InputConflict(input.clone()),
                ));
            }
            spender_changes.push((input.clone(), Some(hash.clone())));
        }
        for parent in &parents {
            causal_edge_insertions.push(CausalEdge {
                parent: parent.clone(),
                child: hash.clone(),
            });
        }
        for child in &children {
            causal_edge_insertions.push(CausalEdge {
                parent: hash.clone(),
                child: child.clone(),
            });
        }

        causal_edge_removals.sort_unstable();
        causal_edge_removals.dedup();
        causal_edge_insertions.sort_unstable();
        causal_edge_insertions.dedup();

        self.reserve_membership_owner_insertions(footprint.inputs().iter(), std::iter::once(hash))?;

        let causal_node_insertions =
            self.prepare_causal_edge_capacity(hash, &parents, &children, &causal_edge_insertions)?;

        // Candidate ownership must dominate the released victim for a shared
        // input. Put `Some(candidate)` before `None`, then keep the first row
        // for each canonical input without a temporary hash table.
        spender_changes.sort_unstable_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.is_none().cmp(&right.1.is_none()))
        });
        spender_changes.dedup_by(|later, earlier| later.0 == earlier.0);

        let mut causal_node_removals = Vec::new();
        causal_node_removals
            .try_reserve(removals.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        causal_node_removals.extend(removals.iter().map(|removal| removal.hash.clone()));
        causal_node_removals.sort_unstable();
        let causal_changes = causal_change_log(
            causal_edge_removals,
            causal_node_insertions,
            causal_edge_insertions,
            causal_node_removals,
        )?;

        ProjectionDelta {
            spender_changes,
            causal_changes,
            ancestor_changes: aggregate.ancestor_changes,
            aggregate_changes: aggregate.changes,
            accepted_order_removals: aggregate.accepted_order_removals,
            accepted_order_insertions: aggregate.accepted_order_insertions,
            eviction_removals: aggregate.eviction_removals,
            eviction_insertions: aggregate.eviction_insertions,
            proposed_counts,
            prestate: Default::default(),
            read_witness: MembershipPolicyWitness::default(),
        }
        .seal_prestate(&self.membership)
    }

    fn prepare_causal_edge_capacity(
        &self,
        hash: &RawTxHash,
        parents: &HashSet<RawTxHash>,
        children: &HashSet<RawTxHash>,
        insertions: &[CausalEdge],
    ) -> Result<Vec<PreparedCausalNode>, super::PlanError> {
        if self.membership.contains_parent_node(hash) || self.membership.contains_child_node(hash) {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        for edge in insertions {
            if &edge.parent != hash {
                let existing =
                    self.membership
                        .children(&edge.parent)
                        .ok_or(super::PlanError::Fault(
                            super::AuthorityFault::MembershipProjection,
                        ))?;
                if existing.contains(&edge.child) {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                self.reserve_membership_child_row(&edge.parent, 1)?;
            }
            if &edge.child != hash {
                let existing =
                    self.membership
                        .parents(&edge.child)
                        .ok_or(super::PlanError::Fault(
                            super::AuthorityFault::MembershipProjection,
                        ))?;
                if existing.contains(&edge.parent) {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                self.reserve_membership_parent_row(&edge.child, 1)?;
            }
        }
        let mut prepared_parents = HashSet::new();
        prepared_parents
            .try_reserve(parents.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut prepared_children = HashSet::new();
        prepared_children
            .try_reserve(children.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut prepared = Vec::new();
        prepared
            .try_reserve_exact(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        prepared.push(PreparedCausalNode {
            hash: hash.clone(),
            parents: prepared_parents,
            children: prepared_children,
        });
        Ok(prepared)
    }

    fn accepted_entry(
        &self,
        hash: &RawTxHash,
    ) -> Result<ShardedAcceptedReadGuard<'_>, super::PlanError> {
        self.entries
            .get(hash)
            .and_then(|owner| owner.into_accepted().ok())
            .ok_or(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ))
    }

    fn bounded_descendant_postorder_with_reader<Mode>(
        roots: &[RawTxHash],
        excluded: &HashSet<RawTxHash>,
        remaining_limit: usize,
        limit_kind: ComponentLimitKind,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<BoundedDescendantPostorder, super::PlanError>
    where
        Mode: PolicyMode,
    {
        let config = reader.config();
        // Mark on enqueue so a high-fanout DAG cannot allocate an attacker-
        // sized frontier before the component limit is observed. Traversal
        // order is irrelevant; the fallibly reserved minimum heap below fixes
        // removal order.
        let mut closure = HashSet::new();
        let mut frontier = VecDeque::new();
        frontier
            .try_reserve(roots.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        for root in roots {
            if excluded.contains(root) || closure.contains(root) {
                continue;
            }
            if closure.len() == remaining_limit {
                return Err(super::PlanError::Membership(
                    MembershipReject::ComponentLimit {
                        kind: limit_kind,
                        limit: config.max_component,
                    },
                ));
            }
            closure
                .try_reserve(1)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            closure.insert(root.clone());
            frontier.push_back(root.clone());
        }
        let mut observed_children = HashMap::new();
        while let Some(hash) = frontier.pop_front() {
            let children = reader
                .observe_children(&hash)?
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            for child in &children {
                if excluded.contains(child) || closure.contains(child) {
                    continue;
                }
                if closure.len() == remaining_limit {
                    return Err(super::PlanError::Membership(
                        MembershipReject::ComponentLimit {
                            kind: limit_kind,
                            limit: config.max_component,
                        },
                    ));
                }
                closure
                    .try_reserve(1)
                    .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
                frontier
                    .try_reserve(1)
                    .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
                closure.insert(child.clone());
                frontier.push_back(child.clone());
            }
            observed_children
                .try_reserve(1)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            if observed_children.insert(hash, children).is_some() {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
        }

        let mut remaining_children = HashMap::new();
        remaining_children
            .try_reserve(closure.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut leaves = BinaryHeap::new();
        leaves
            .try_reserve(closure.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        for hash in &closure {
            let children = observed_children.get(hash).ok_or(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ))?;
            let count = children
                .iter()
                .filter(|child| closure.contains(*child))
                .count();
            remaining_children.insert(hash.clone(), count);
            if count == 0 {
                leaves.push(Reverse(hash.clone()));
            }
        }
        let mut ordered = Vec::new();
        ordered
            .try_reserve(closure.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        while let Some(Reverse(hash)) = leaves.pop() {
            ordered.push(hash.clone());
            let parents = reader
                .observe_parents(&hash)?
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            for parent in parents {
                if !closure.contains(&parent) {
                    continue;
                }
                let count = remaining_children
                    .get_mut(&parent)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
                *count = count.checked_sub(1).ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
                if *count == 0 {
                    leaves.push(Reverse(parent.clone()));
                }
            }
        }
        if ordered.len() != closure.len() {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        let mut child_rows = Vec::new();
        child_rows
            .try_reserve_exact(observed_children.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        child_rows.extend(observed_children);
        child_rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        Ok(BoundedDescendantPostorder {
            ordered,
            child_rows,
        })
    }

    fn candidate_parents<Mode>(
        candidate: &AcceptedEntry,
        removed: &HashSet<RawTxHash>,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError>
    where
        Mode: PolicyMode,
    {
        let footprint = &candidate.proof.payload().footprint;
        let mut parents = HashSet::new();
        parents
            .try_reserve(footprint.edge_count())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        for out_point in footprint.inputs().iter().chain(footprint.dependencies()) {
            if let Some(parent) = Self::surviving_pool_parent(out_point, removed, reader)? {
                parents.insert(parent);
            }
        }
        Ok(parents)
    }

    fn surviving_pool_parent<Mode>(
        out_point: &OutPoint,
        removed: &HashSet<RawTxHash>,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<Option<RawTxHash>, super::PlanError>
    where
        Mode: PolicyMode,
    {
        let parent = RawTxHash(out_point.tx_hash());
        let fact = reader.observe_owner(&parent)?;
        if !matches!(
            fact,
            Some(OwnerReadFact {
                kind: OwnerEntryKind::Accepted,
                ..
            })
        ) {
            return Ok(None);
        }
        let entry = reader.observe_accepted_owner(&parent)?;
        if removed.contains(&parent) {
            return Ok(None);
        }
        let index: u32 = out_point.index().unpack();
        let output_exists = usize::try_from(index)
            .is_ok_and(|index| index < entry.record.tx.data().raw().outputs().len());
        if !output_exists {
            return Err(super::PlanError::Membership(
                MembershipReject::MissingPoolOutput(out_point.clone()),
            ));
        }
        Ok(Some(parent))
    }

    fn validate_candidate_input_evidence<Mode>(
        candidate: &AcceptedEntry,
        removed: &HashSet<RawTxHash>,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<(), super::PlanError>
    where
        Mode: PolicyMode,
    {
        // This is the final membership proof, not another liveness query.
        // Every input must carry positive same-epoch chain evidence, name an
        // exact surviving pool output, or be released by this RBF Plan.
        for input in candidate.proof.payload().footprint.inputs() {
            let released_by_replacement = reader
                .observe_spender(input)?
                .is_some_and(|spender| removed.contains(&spender));
            if candidate.proof.is_chain_input(input) || released_by_replacement {
                continue;
            }
            if Self::surviving_pool_parent(input, removed, reader)?.is_none() {
                return Err(super::PlanError::Membership(
                    MembershipReject::MissingInputEvidence(input.clone()),
                ));
            }
        }
        Ok(())
    }

    fn validate_candidate_dependency_evidence<Mode>(
        candidate: &AcceptedEntry,
        removed: &HashSet<RawTxHash>,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<(), super::PlanError>
    where
        Mode: PolicyMode,
    {
        // A resolved dependency is either positive chain evidence from the
        // final validation view or an exact surviving Accepted output. This
        // closes the resolver boundary: PreAccepted outputs can never become
        // causal membership merely because a future resolver exposed them.
        for dependency in candidate.proof.payload().footprint.dependencies() {
            if candidate.proof.is_chain_dependency(dependency)
                || Self::surviving_pool_parent(dependency, removed, reader)?.is_some()
            {
                continue;
            }
            return Err(super::PlanError::Membership(
                MembershipReject::MissingDependencyEvidence(dependency.clone()),
            ));
        }
        Ok(())
    }

    fn candidate_children<Mode>(
        candidate: &AcceptedEntry,
        removed: &HashSet<RawTxHash>,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError>
    where
        Mode: PolicyMode,
    {
        let child_limit = reader.config().max_component;
        let mut children = HashSet::new();
        for child in Self::accepted_children_of_candidate(candidate, reader)? {
            if removed.contains(&child) || children.contains(&child) {
                continue;
            }
            if children.len() == child_limit {
                return Err(super::PlanError::Membership(
                    MembershipReject::ComponentLimit {
                        kind: ComponentLimitKind::Mutation,
                        limit: child_limit,
                    },
                ));
            }
            reader.observe_accepted_owner(&child)?;
            children
                .try_reserve(1)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            children.insert(child.clone());
        }
        Ok(children)
    }

    fn accepted_children_of_candidate<Mode>(
        candidate: &AcceptedEntry,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<Vec<RawTxHash>, super::PlanError>
    where
        Mode: PolicyMode,
    {
        let outputs = candidate.record.tx.output_pts();
        let mut children = Vec::new();
        children
            .try_reserve(outputs.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        for output in outputs {
            if let Some(spender) = reader.observe_spender(&output)? {
                children.push(spender);
            }
            children.extend(Self::accepted_dependency_consumers_for_policy(
                &output, reader,
            )?);
        }
        Ok(children)
    }

    fn accepted_dependency_consumers_for_policy<Mode>(
        dependency: &OutPoint,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError>
    where
        Mode: PolicyMode,
    {
        Self::accepted_dependency_consumers_with_reader(dependency, reader)
    }

    fn accepted_dependency_consumers_with_reader<Mode>(
        dependency: &OutPoint,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError>
    where
        Mode: PolicyMode,
    {
        let key = DependencyKey::Cell(dependency.clone());
        let consumers = reader
            .observe_dependency_consumers(key.clone())?
            .unwrap_or_default();
        let mut accepted = HashSet::new();
        accepted
            .try_reserve(consumers.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        for hash in consumers {
            let (has_dependency, is_accepted) = reader.observe_dependency_owner(&hash, &key)?;
            if !has_dependency {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            if is_accepted {
                accepted.insert(hash);
            }
        }
        Ok(accepted)
    }

    fn collect_surviving_ancestors(
        &self,
        parents: &HashSet<RawTxHash>,
        removed: &HashSet<RawTxHash>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError> {
        let mut reader = PolicyContext::exclusive(self);
        Self::collect_surviving_ancestors_with_reader(parents, removed, &mut reader)
    }

    fn collect_surviving_ancestors_with_reader<Mode>(
        parents: &HashSet<RawTxHash>,
        removed: &HashSet<RawTxHash>,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError>
    where
        Mode: PolicyMode,
    {
        let max_ancestors = reader.config().max_ancestors;
        let mut ancestors = HashSet::new();
        let mut frontier = VecDeque::new();
        frontier
            .try_reserve(parents.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        frontier.extend(parents.iter().cloned());
        while let Some(ancestor) = frontier.pop_front() {
            if removed.contains(&ancestor) || ancestors.contains(&ancestor) {
                continue;
            }
            ancestors
                .try_reserve(1)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            ancestors.insert(ancestor.clone());
            if ancestors.len() >= max_ancestors {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            let parents = reader
                .observe_parents(&ancestor)?
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            frontier.extend(parents.iter().cloned());
        }
        Ok(ancestors)
    }

    /// Return every surviving ancestor that previously counted `descendant`.
    /// Unlike final-graph ancestry, aggregate subtraction must traverse
    /// through removed intermediate nodes: each removed descendant was part
    /// of every such ancestor's pre-transition aggregate.
    fn collect_surviving_ancestors_through_removals(
        &self,
        descendant: &RawTxHash,
        removed: &HashSet<RawTxHash>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError> {
        let mut reader = PolicyContext::exclusive(self);
        Self::collect_surviving_ancestors_through_removals_with_reader(
            descendant,
            removed,
            &mut reader,
        )
    }

    fn collect_surviving_ancestors_through_removals_with_reader<Mode>(
        descendant: &RawTxHash,
        removed: &HashSet<RawTxHash>,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError>
    where
        Mode: PolicyMode,
    {
        let max_ancestors = reader.config().max_ancestors;
        let parents = reader
            .observe_parents(descendant)?
            .ok_or(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ))?;
        let mut ancestors = HashSet::new();
        let mut visited = HashSet::new();
        let mut frontier = VecDeque::new();
        frontier
            .try_reserve(parents.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        frontier.extend(parents.iter().cloned());
        while let Some(ancestor) = frontier.pop_front() {
            visited
                .try_reserve(1)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            if !visited.insert(ancestor.clone()) {
                continue;
            }
            if visited.len() >= max_ancestors {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            if !removed.contains(&ancestor) {
                ancestors
                    .try_reserve(1)
                    .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
                ancestors.insert(ancestor.clone());
            }
            let grandparents =
                reader
                    .observe_parents(&ancestor)?
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
            frontier
                .try_reserve(grandparents.len())
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            frontier.extend(grandparents.iter().cloned());
        }
        Ok(ancestors)
    }

    fn candidate_ancestors<Mode>(
        parents: &HashSet<RawTxHash>,
        removed: &HashSet<RawTxHash>,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError>
    where
        Mode: PolicyMode,
    {
        let max_ancestors = reader.config().max_ancestors;
        let mut ancestors = HashSet::new();
        let mut frontier = VecDeque::new();
        frontier
            .try_reserve(parents.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        frontier.extend(parents.iter().cloned());
        while let Some(parent) = frontier.pop_front() {
            if removed.contains(&parent) || ancestors.contains(&parent) {
                continue;
            }
            ancestors
                .try_reserve(1)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            ancestors.insert(parent.clone());
            // The candidate itself consumes one configured ancestor slot.
            if ancestors.len() >= max_ancestors {
                return Err(super::PlanError::Membership(
                    MembershipReject::TooManyAncestors,
                ));
            }
            let grandparents = reader
                .observe_parents(&parent)?
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            frontier.extend(grandparents.iter().cloned());
        }
        Ok(ancestors)
    }
}
