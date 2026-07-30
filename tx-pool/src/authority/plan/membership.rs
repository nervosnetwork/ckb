mod eviction;
mod independent;
mod rbf;

pub(in crate::authority) use independent::IndependentCoupling;
pub(in crate::authority::plan) use independent::{
    IndependentMembershipChange, IndependentMembershipOutcome, PreparedIndependentMembership,
    prepare_independent_membership,
};

use super::TxPoolAuthority;
use crate::authority::{
    resources::{AcceptedCost, ResourceBatchPlan, ResourceError},
    state::{
        AcceptedEntry, AcceptedStatus, Arrival, OwnedTx, PreAcceptedEntry, ProposalId, RawTxHash,
    },
};
use ckb_types::{
    core::{Capacity, FeeRate, tx_pool::get_transaction_weight},
    packed::OutPoint,
    prelude::Unpack,
};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug)]
enum ReplacementPolicy {
    Disabled,
    Enabled { minimum_rate: FeeRate },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MembershipConfig {
    max_ancestors: usize,
    max_component: usize,
    replacement: ReplacementPolicy,
}

impl MembershipConfig {
    pub(super) fn testing_default() -> Self {
        Self {
            max_ancestors: 125,
            max_component: crate::constants::MAX_POOL_MUTATION_CANDIDATES,
            replacement: ReplacementPolicy::Disabled,
        }
    }

    #[cfg(test)]
    pub(super) fn testing_with_replacement(minimum_rate: FeeRate) -> Self {
        Self {
            replacement: ReplacementPolicy::Enabled { minimum_rate },
            ..Self::testing_default()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct StatusCounts {
    pub(in crate::authority) pending: usize,
    pub(in crate::authority) gap: usize,
    pub(in crate::authority) proposed: usize,
}

impl StatusCounts {
    fn checked_add(self, status: AcceptedStatus) -> Option<Self> {
        let mut next = self;
        let count = next.for_status_mut(status);
        *count = count.checked_add(1)?;
        Some(next)
    }

    fn checked_sub(self, status: AcceptedStatus) -> Option<Self> {
        let mut next = self;
        let count = next.for_status_mut(status);
        *count = count.checked_sub(1)?;
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

#[derive(Debug, Default)]
pub(super) struct MembershipProjection {
    spenders: HashMap<OutPoint, RawTxHash>,
    dependency_readers: HashMap<OutPoint, HashSet<RawTxHash>>,
    parents: HashMap<RawTxHash, HashSet<RawTxHash>>,
    children: HashMap<RawTxHash, HashSet<RawTxHash>>,
    descendant_aggregates: HashMap<RawTxHash, DescendantAggregate>,
    eviction_order: BTreeSet<EvictionOrderKey>,
    counts: StatusCounts,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct DescendantAggregate {
    pub(in crate::authority) entries: usize,
    pub(in crate::authority) serialized_bytes: usize,
    pub(in crate::authority) cycles: u64,
    pub(in crate::authority) fee: Capacity,
}

impl DescendantAggregate {
    fn one(entry: &AcceptedEntry) -> Self {
        let cost = entry.verified.metrics().cost;
        Self {
            entries: 1,
            serialized_bytes: cost.serialized_bytes,
            cycles: cost.cycles,
            fee: entry.verified.metrics().fee,
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::authority) struct EvictionOrderKey {
    pub(in crate::authority) status: AcceptedStatus,
    pub(in crate::authority) fee_rate: FeeRate,
    pub(in crate::authority) descendants_count: usize,
    pub(in crate::authority) arrival: Arrival,
    pub(in crate::authority) hash: RawTxHash,
}

impl EvictionOrderKey {
    fn new(entry: &AcceptedEntry, aggregate: DescendantAggregate) -> Self {
        let AcceptedCost {
            serialized_bytes,
            cycles,
            ..
        } = entry.verified.metrics().cost;
        let self_rate = FeeRate::calculate(
            entry.verified.metrics().fee,
            get_transaction_weight(serialized_bytes, cycles),
        );
        let descendants_rate = FeeRate::calculate(
            aggregate.fee,
            get_transaction_weight(aggregate.serialized_bytes, aggregate.cycles),
        );
        Self {
            status: entry.status,
            fee_rate: self_rate.max(descendants_rate),
            descendants_count: aggregate.entries,
            arrival: entry.record.arrival,
            hash: entry.record.identity.raw.clone(),
        }
    }
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct MembershipSnapshot {
    pub(in crate::authority) spenders: HashMap<OutPoint, RawTxHash>,
    pub(in crate::authority) dependency_readers: HashMap<OutPoint, HashSet<RawTxHash>>,
    pub(in crate::authority) parents: HashMap<RawTxHash, HashSet<RawTxHash>>,
    pub(in crate::authority) children: HashMap<RawTxHash, HashSet<RawTxHash>>,
    pub(in crate::authority) descendant_aggregates: HashMap<RawTxHash, DescendantAggregate>,
    pub(in crate::authority) eviction_order: BTreeSet<EvictionOrderKey>,
    pub(in crate::authority) counts: StatusCounts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) enum MembershipReject {
    InputConflict(OutPoint),
    TooManyAncestors,
    ComponentLimit {
        limit: usize,
    },
    NewUnconfirmedInput(OutPoint),
    InputFromDescendant(OutPoint),
    AncestorDescendantOverlap,
    DependencyOnVictim(OutPoint),
    InsufficientReplacementFee {
        actual: ckb_types::core::Capacity,
        required: ckb_types::core::Capacity,
    },
    ReplacementFeeOverflow,
    AggregateOverflow,
    CandidateEvicted,
    CausalCycle(RawTxHash),
    MissingInputEvidence(OutPoint),
    MissingPoolOutput(OutPoint),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct MembershipRemoval {
    pub(in crate::authority) hash: RawTxHash,
    pub(in crate::authority) proposal: ProposalId,
    pub(in crate::authority) cause: RemovalCause,
}

pub(super) struct PreparedMembership {
    pub(super) removals: Vec<MembershipRemoval>,
    pub(super) resource: ResourceBatchPlan,
    pub(super) projection: ProjectionDelta,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct DependencyReaderEdge {
    dependency: OutPoint,
    reader: RawTxHash,
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

struct PreparedDependencyRow {
    dependency: OutPoint,
    readers: HashSet<RawTxHash>,
}

/// Ordered relation journals replace whole-row snapshots. Plan reserves every
/// touched row first; Apply then consumes edge changes in this order.
enum DependencyRelationChange {
    RemoveEdge(DependencyReaderEdge),
    InsertRow(PreparedDependencyRow),
    InsertEdge(DependencyReaderEdge),
    RemoveRow(OutPoint),
}

/// A causal edge is one semantic change even though it maintains both lookup
/// directions, so callers cannot publish `parents` without `children`.
enum CausalRelationChange {
    RemoveEdge(CausalEdge),
    InsertNode(PreparedCausalNode),
    InsertEdge(CausalEdge),
    RemoveNode(RawTxHash),
}

fn dependency_change_log(
    removals: Vec<DependencyReaderEdge>,
    row_insertions: Vec<PreparedDependencyRow>,
    insertions: Vec<DependencyReaderEdge>,
    row_removals: Vec<OutPoint>,
) -> Result<Vec<DependencyRelationChange>, super::PlanError> {
    let capacity = removals
        .len()
        .checked_add(row_insertions.len())
        .and_then(|count| count.checked_add(insertions.len()))
        .and_then(|count| count.checked_add(row_removals.len()))
        .ok_or(super::PlanError::Fault(
            super::AuthorityFault::CounterExhausted,
        ))?;
    let mut changes = Vec::new();
    changes
        .try_reserve(capacity)
        .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
    changes.extend(
        removals
            .into_iter()
            .map(DependencyRelationChange::RemoveEdge),
    );
    changes.extend(
        row_insertions
            .into_iter()
            .map(DependencyRelationChange::InsertRow),
    );
    changes.extend(
        insertions
            .into_iter()
            .map(DependencyRelationChange::InsertEdge),
    );
    changes.extend(
        row_removals
            .into_iter()
            .map(DependencyRelationChange::RemoveRow),
    );
    Ok(changes)
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
    dependency_changes: Vec<DependencyRelationChange>,
    causal_changes: Vec<CausalRelationChange>,
    aggregate_changes: Vec<(RawTxHash, Option<DescendantAggregate>)>,
    eviction_removals: Vec<EvictionOrderKey>,
    eviction_insertions: Vec<EvictionOrderKey>,
    counts: StatusCounts,
}

struct AggregateDelta {
    changes: Vec<(RawTxHash, Option<DescendantAggregate>)>,
    eviction_removals: Vec<EvictionOrderKey>,
    eviction_insertions: Vec<EvictionOrderKey>,
}

struct EvictionPlan {
    removals: Vec<SelectedRemoval>,
    candidate_parents: HashSet<RawTxHash>,
    candidate_children: HashSet<RawTxHash>,
    aggregate: AggregateDelta,
}

impl MembershipProjection {
    pub(super) fn counts(&self) -> StatusCounts {
        self.counts
    }

    pub(super) fn spender(&self, input: &OutPoint) -> Option<&RawTxHash> {
        self.spenders.get(input)
    }

    fn dependency_readers(&self, dependency: &OutPoint) -> Option<&HashSet<RawTxHash>> {
        self.dependency_readers.get(dependency)
    }

    pub(super) fn parents(&self, hash: &RawTxHash) -> Option<&HashSet<RawTxHash>> {
        self.parents.get(hash)
    }

    pub(super) fn children(&self, hash: &RawTxHash) -> Option<&HashSet<RawTxHash>> {
        self.children.get(hash)
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> MembershipSnapshot {
        MembershipSnapshot {
            spenders: self.spenders.clone(),
            dependency_readers: self.dependency_readers.clone(),
            parents: self.parents.clone(),
            children: self.children.clone(),
            descendant_aggregates: self.descendant_aggregates.clone(),
            eviction_order: self.eviction_order.clone(),
            counts: self.counts,
        }
    }

    pub(super) fn apply(&mut self, delta: ProjectionDelta) {
        for (input, spender) in delta.spender_changes {
            match spender {
                Some(spender) => {
                    self.spenders.insert(input, spender);
                }
                None => {
                    self.spenders.remove(&input);
                }
            }
        }
        for change in delta.dependency_changes {
            match change {
                DependencyRelationChange::RemoveEdge(edge) => {
                    if let Some(readers) = self.dependency_readers.get_mut(&edge.dependency) {
                        readers.remove(&edge.reader);
                    }
                }
                DependencyRelationChange::InsertRow(row) => {
                    self.dependency_readers.insert(row.dependency, row.readers);
                }
                DependencyRelationChange::InsertEdge(edge) => {
                    if let Some(readers) = self.dependency_readers.get_mut(&edge.dependency) {
                        readers.insert(edge.reader);
                    }
                }
                DependencyRelationChange::RemoveRow(dependency) => {
                    self.dependency_readers.remove(&dependency);
                }
            }
        }
        for change in delta.causal_changes {
            match change {
                CausalRelationChange::RemoveEdge(edge) => {
                    if let Some(children) = self.children.get_mut(&edge.parent) {
                        children.remove(&edge.child);
                    }
                    if let Some(parents) = self.parents.get_mut(&edge.child) {
                        parents.remove(&edge.parent);
                    }
                }
                CausalRelationChange::InsertNode(node) => {
                    self.parents.insert(node.hash.clone(), node.parents);
                    self.children.insert(node.hash, node.children);
                }
                CausalRelationChange::InsertEdge(edge) => {
                    if let Some(children) = self.children.get_mut(&edge.parent) {
                        children.insert(edge.child.clone());
                    }
                    if let Some(parents) = self.parents.get_mut(&edge.child) {
                        parents.insert(edge.parent);
                    }
                }
                CausalRelationChange::RemoveNode(hash) => {
                    self.parents.remove(&hash);
                    self.children.remove(&hash);
                }
            }
        }
        for (hash, aggregate) in delta.aggregate_changes {
            match aggregate {
                Some(aggregate) => {
                    self.descendant_aggregates.insert(hash, aggregate);
                }
                None => {
                    self.descendant_aggregates.remove(&hash);
                }
            }
        }
        for key in delta.eviction_removals {
            self.eviction_order.remove(&key);
        }
        for key in delta.eviction_insertions {
            self.eviction_order.insert(key);
        }
        self.counts = delta.counts;
    }
}

impl TxPoolAuthority {
    pub(super) fn prepare_membership(
        &mut self,
        hash: &RawTxHash,
        before: &PreAcceptedEntry,
        candidate: &AcceptedEntry,
    ) -> Result<PreparedMembership, super::PlanError> {
        if before.record.identity.raw != *hash
            || candidate.record.identity != before.record.identity
            || candidate.record.ingress != before.record.ingress
            || candidate.record.blame != before.record.blame
            || candidate.record.class != before.record.class
            || candidate.record.arrival != before.record.arrival
        {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        let mandatory = rbf::replacement_removals(self, candidate)?;
        let EvictionPlan {
            removals: selected_removals,
            candidate_parents,
            candidate_children,
            aggregate,
        } = eviction::complete_removals(self, hash, candidate, mandatory)?;
        let projection = self.prepare_projection_change(
            hash,
            candidate,
            &selected_removals,
            candidate_parents,
            candidate_children,
            aggregate,
        )?;
        let (removals, resource) =
            self.prepare_membership_resources(hash, before, candidate, selected_removals)?;
        Ok(PreparedMembership {
            removals,
            resource,
            projection,
        })
    }

    fn prepare_membership_resources(
        &mut self,
        hash: &RawTxHash,
        before: &PreAcceptedEntry,
        candidate: &AcceptedEntry,
        selected_removals: Vec<SelectedRemoval>,
    ) -> Result<(Vec<MembershipRemoval>, ResourceBatchPlan), super::PlanError> {
        let current = self.entries.get(hash).ok_or(super::PlanError::Fault(
            super::AuthorityFault::MembershipProjection,
        ))?;
        if !matches!(current, OwnedTx::PreAccepted(entry) if
            entry.record.version == before.record.version
                && entry.record.identity == before.record.identity)
        {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }

        let change_capacity =
            selected_removals
                .len()
                .checked_add(1)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
        let mut resource_changes = Vec::new();
        resource_changes
            .try_reserve(change_capacity)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        resource_changes.push((
            hash.clone(),
            Some(before.charge_record()),
            Some(candidate.charge_record()),
        ));

        let mut removals = Vec::new();
        removals
            .try_reserve(selected_removals.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        for selected in selected_removals {
            let victim = self.accepted_entry(&selected.hash)?;
            resource_changes.push((selected.hash.clone(), Some(victim.charge_record()), None));
            removals.push(MembershipRemoval {
                hash: selected.hash,
                proposal: victim.record.identity.proposal.clone(),
                cause: selected.cause,
            });
        }

        let resource =
            self.resources
                .plan_batch(resource_changes)
                .map_err(|error| match error {
                    ResourceError::Allocation => {
                        super::PlanError::Backpressure(super::Backpressure::Allocation)
                    }
                    ResourceError::Arithmetic
                    | ResourceError::PreAcceptedLimit
                    | ResourceError::RemoteLimit
                    | ResourceError::PeerLimit(_)
                    | ResourceError::AcceptedLimit
                    | ResourceError::ExistingChargeMismatch
                    | ResourceError::DuplicateChange
                    | ResourceError::ComputeEnvelope
                    | ResourceError::AttributionMismatch => {
                        super::PlanError::Fault(super::AuthorityFault::ResourceProjection)
                    }
                })?;
        Ok((removals, resource))
    }

    pub(super) fn prepare_status_change(
        &self,
        hash: &RawTxHash,
        before: &AcceptedEntry,
        after: &AcceptedEntry,
    ) -> Result<ProjectionDelta, super::PlanError> {
        if before.record.identity.raw != *hash
            || after.record.identity.raw != *hash
            || before.status == after.status
        {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        let counts = self
            .membership
            .counts
            .checked_sub(before.status)
            .and_then(|counts| counts.checked_add(after.status))
            .ok_or(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ))?;
        let aggregate = self
            .membership
            .descendant_aggregates
            .get(hash)
            .copied()
            .ok_or(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ))?;
        let previous_key = EvictionOrderKey::new(before, aggregate);
        if !self.membership.eviction_order.contains(&previous_key) {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        let mut eviction_removals = Vec::new();
        eviction_removals
            .try_reserve(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        eviction_removals.push(previous_key);
        let mut eviction_insertions = Vec::new();
        eviction_insertions
            .try_reserve(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        eviction_insertions.push(EvictionOrderKey::new(after, aggregate));
        Ok(ProjectionDelta {
            spender_changes: Vec::new(),
            dependency_changes: Vec::new(),
            causal_changes: Vec::new(),
            aggregate_changes: Vec::new(),
            eviction_removals,
            eviction_insertions,
            counts,
        })
    }

    fn prepare_projection_change(
        &mut self,
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
        let mut counts = self.membership.counts;
        for planned in removals {
            let removal = &planned.hash;
            let entry = self.accepted_entry(removal)?;
            counts = counts
                .checked_sub(entry.status)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
        }
        counts = counts
            .checked_add(candidate.status)
            .ok_or(super::PlanError::Fault(
                super::AuthorityFault::CounterExhausted,
            ))?;

        let footprint = &candidate.verified.payload().footprint;
        let mut removal_inputs = 0usize;
        let mut removal_dependencies = 0usize;
        let mut removal_causal_edges = 0usize;
        for planned in removals {
            let entry = self.accepted_entry(&planned.hash)?;
            removal_inputs = removal_inputs
                .checked_add(entry.verified.payload().footprint.inputs().len())
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
            removal_dependencies = removal_dependencies
                .checked_add(entry.verified.payload().footprint.dependencies().len())
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
        let mut spender_after = HashMap::new();
        spender_after
            .try_reserve(spender_capacity)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut dependency_reader_removals = Vec::new();
        dependency_reader_removals
            .try_reserve(removal_dependencies)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut dependency_reader_insertions = Vec::new();
        dependency_reader_insertions
            .try_reserve(footprint.dependencies().len())
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
            for input in entry.verified.payload().footprint.inputs() {
                if self.membership.spender(input) != Some(removal) {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                spender_after.insert(input.clone(), None);
            }
            for dependency in entry.verified.payload().footprint.dependencies() {
                let readers = self.membership.dependency_readers(dependency).ok_or(
                    super::PlanError::Fault(super::AuthorityFault::MembershipProjection),
                )?;
                if !readers.contains(removal) {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                dependency_reader_removals.push(DependencyReaderEdge {
                    dependency: dependency.clone(),
                    reader: removal.clone(),
                });
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
                        .children(parent)
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
                        .parents(child)
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
                .is_some_and(|spender| !removed.contains(spender))
            {
                return Err(super::PlanError::Membership(
                    MembershipReject::InputConflict(input.clone()),
                ));
            }
            spender_after.insert(input.clone(), Some(hash.clone()));
        }
        for dependency in footprint.dependencies() {
            dependency_reader_insertions.push(DependencyReaderEdge {
                dependency: dependency.clone(),
                reader: hash.clone(),
            });
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

        dependency_reader_removals.sort_unstable();
        dependency_reader_removals.dedup();
        dependency_reader_insertions.sort_unstable();
        dependency_reader_insertions.dedup();
        causal_edge_removals.sort_unstable();
        causal_edge_removals.dedup();
        causal_edge_insertions.sort_unstable();
        causal_edge_insertions.dedup();

        self.membership
            .spenders
            .try_reserve(footprint.inputs().len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        self.membership
            .parents
            .try_reserve(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        self.membership
            .children
            .try_reserve(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        self.membership
            .descendant_aggregates
            .try_reserve(1)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;

        let (dependency_row_insertions, dependency_row_removals) = self
            .prepare_dependency_edge_capacity(
                &dependency_reader_removals,
                &dependency_reader_insertions,
            )?;
        let causal_node_insertions =
            self.prepare_causal_edge_capacity(hash, &parents, &children, &causal_edge_insertions)?;

        let mut spender_changes = Vec::new();
        spender_changes
            .try_reserve(spender_after.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        spender_changes.extend(spender_after);
        spender_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));

        let mut causal_node_removals = Vec::new();
        causal_node_removals
            .try_reserve(removals.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        causal_node_removals.extend(removals.iter().map(|removal| removal.hash.clone()));
        causal_node_removals.sort_unstable();
        let dependency_changes = dependency_change_log(
            dependency_reader_removals,
            dependency_row_insertions,
            dependency_reader_insertions,
            dependency_row_removals,
        )?;
        let causal_changes = causal_change_log(
            causal_edge_removals,
            causal_node_insertions,
            causal_edge_insertions,
            causal_node_removals,
        )?;

        Ok(ProjectionDelta {
            spender_changes,
            dependency_changes,
            causal_changes,
            aggregate_changes: aggregate.changes,
            eviction_removals: aggregate.eviction_removals,
            eviction_insertions: aggregate.eviction_insertions,
            counts,
        })
    }

    fn prepare_dependency_edge_capacity(
        &mut self,
        removals: &[DependencyReaderEdge],
        insertions: &[DependencyReaderEdge],
    ) -> Result<(Vec<PreparedDependencyRow>, Vec<OutPoint>), super::PlanError> {
        let edge_count =
            removals
                .len()
                .checked_add(insertions.len())
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
        let mut counts = HashMap::<OutPoint, (usize, usize)>::new();
        counts
            .try_reserve(edge_count)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        for edge in removals {
            let readers = self.membership.dependency_readers(&edge.dependency).ok_or(
                super::PlanError::Fault(super::AuthorityFault::MembershipProjection),
            )?;
            if !readers.contains(&edge.reader) {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            let count = counts.entry(edge.dependency.clone()).or_default();
            count.0 = count.0.checked_add(1).ok_or(super::PlanError::Fault(
                super::AuthorityFault::CounterExhausted,
            ))?;
        }
        for edge in insertions {
            if self
                .membership
                .dependency_readers(&edge.dependency)
                .is_some_and(|readers| readers.contains(&edge.reader))
            {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            let count = counts.entry(edge.dependency.clone()).or_default();
            count.1 = count.1.checked_add(1).ok_or(super::PlanError::Fault(
                super::AuthorityFault::CounterExhausted,
            ))?;
        }

        let new_rows = counts
            .keys()
            .filter(|dependency| !self.membership.dependency_readers.contains_key(*dependency))
            .count();
        self.membership
            .dependency_readers
            .try_reserve(new_rows)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut row_insertions = Vec::new();
        row_insertions
            .try_reserve(new_rows)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut row_removals = Vec::new();
        row_removals
            .try_reserve(counts.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut ordered_counts = counts.into_iter().collect::<Vec<_>>();
        ordered_counts.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        for (dependency, (remove_count, insert_count)) in ordered_counts {
            match self.membership.dependency_readers.get_mut(&dependency) {
                Some(readers) => {
                    let remaining =
                        readers
                            .len()
                            .checked_sub(remove_count)
                            .ok_or(super::PlanError::Fault(
                                super::AuthorityFault::MembershipProjection,
                            ))?;
                    let final_count =
                        remaining
                            .checked_add(insert_count)
                            .ok_or(super::PlanError::Fault(
                                super::AuthorityFault::CounterExhausted,
                            ))?;
                    readers.try_reserve(insert_count).map_err(|_| {
                        super::PlanError::Backpressure(super::Backpressure::Allocation)
                    })?;
                    if final_count == 0 {
                        row_removals.push(dependency);
                    }
                }
                None => {
                    if remove_count != 0 || insert_count == 0 {
                        return Err(super::PlanError::Fault(
                            super::AuthorityFault::MembershipProjection,
                        ));
                    }
                    let mut readers = HashSet::new();
                    readers.try_reserve(insert_count).map_err(|_| {
                        super::PlanError::Backpressure(super::Backpressure::Allocation)
                    })?;
                    row_insertions.push(PreparedDependencyRow {
                        dependency,
                        readers,
                    });
                }
            }
        }
        Ok((row_insertions, row_removals))
    }

    fn prepare_causal_edge_capacity(
        &mut self,
        hash: &RawTxHash,
        parents: &HashSet<RawTxHash>,
        children: &HashSet<RawTxHash>,
        insertions: &[CausalEdge],
    ) -> Result<Vec<PreparedCausalNode>, super::PlanError> {
        if self.membership.parents.contains_key(hash) || self.membership.children.contains_key(hash)
        {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        for edge in insertions {
            if &edge.parent != hash {
                let existing = self.membership.children.get_mut(&edge.parent).ok_or(
                    super::PlanError::Fault(super::AuthorityFault::MembershipProjection),
                )?;
                if existing.contains(&edge.child) {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                existing
                    .try_reserve(1)
                    .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            }
            if &edge.child != hash {
                let existing =
                    self.membership
                        .parents
                        .get_mut(&edge.child)
                        .ok_or(super::PlanError::Fault(
                            super::AuthorityFault::MembershipProjection,
                        ))?;
                if existing.contains(&edge.parent) {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                existing
                    .try_reserve(1)
                    .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
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
        Ok(vec![PreparedCausalNode {
            hash: hash.clone(),
            parents: prepared_parents,
            children: prepared_children,
        }])
    }

    fn accepted_entry(&self, hash: &RawTxHash) -> Result<&AcceptedEntry, super::PlanError> {
        match self.entries.get(hash) {
            Some(OwnedTx::Accepted(entry)) => Ok(entry),
            Some(OwnedTx::PreAccepted(_)) | None => Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            )),
        }
    }

    fn bounded_descendant_postorder(
        &self,
        roots: &BTreeSet<RawTxHash>,
        excluded: &HashSet<RawTxHash>,
        remaining_limit: usize,
    ) -> Result<Vec<RawTxHash>, super::PlanError> {
        // Mark on enqueue so a high-fanout DAG cannot allocate an attacker-
        // sized frontier before the component limit is observed. Traversal
        // order is irrelevant; the leaf BTreeSet below fixes removal order.
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
                        limit: self.membership_config.max_component,
                    },
                ));
            }
            closure
                .try_reserve(1)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            closure.insert(root.clone());
            frontier.push_back(root.clone());
        }
        while let Some(hash) = frontier.pop_front() {
            let children = self
                .membership
                .children(&hash)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            for child in children {
                if excluded.contains(child) || closure.contains(child) {
                    continue;
                }
                if closure.len() == remaining_limit {
                    return Err(super::PlanError::Membership(
                        MembershipReject::ComponentLimit {
                            limit: self.membership_config.max_component,
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
        }

        let mut remaining_children = HashMap::new();
        remaining_children
            .try_reserve(closure.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut leaves = BTreeSet::new();
        for hash in &closure {
            let children = self
                .membership
                .children(hash)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            let count = children
                .iter()
                .filter(|child| closure.contains(*child))
                .count();
            remaining_children.insert(hash.clone(), count);
            if count == 0 {
                leaves.insert(hash.clone());
            }
        }
        let mut ordered = Vec::new();
        ordered
            .try_reserve(closure.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        while let Some(hash) = leaves.pop_first() {
            ordered.push(hash.clone());
            let parents = self
                .membership
                .parents(&hash)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            for parent in parents {
                if !closure.contains(parent) {
                    continue;
                }
                let count = remaining_children
                    .get_mut(parent)
                    .ok_or(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ))?;
                *count = count.checked_sub(1).ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
                if *count == 0 {
                    leaves.insert(parent.clone());
                }
            }
        }
        if ordered.len() != closure.len() {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        Ok(ordered)
    }

    fn candidate_parents(
        &self,
        candidate: &AcceptedEntry,
        removed: &HashSet<RawTxHash>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError> {
        let footprint = &candidate.verified.payload().footprint;
        let mut parents = HashSet::new();
        parents
            .try_reserve(footprint.edge_count())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        for out_point in footprint.inputs().iter().chain(footprint.dependencies()) {
            if let Some(parent) = self.surviving_pool_parent(out_point, removed)? {
                parents.insert(parent);
            }
        }
        Ok(parents)
    }

    fn surviving_pool_parent(
        &self,
        out_point: &OutPoint,
        removed: &HashSet<RawTxHash>,
    ) -> Result<Option<RawTxHash>, super::PlanError> {
        let parent = RawTxHash(out_point.tx_hash());
        let Some(OwnedTx::Accepted(entry)) = self.entries.get(&parent) else {
            return Ok(None);
        };
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

    fn validate_candidate_input_evidence(
        &self,
        candidate: &AcceptedEntry,
        removed: &HashSet<RawTxHash>,
    ) -> Result<(), super::PlanError> {
        // This is the final membership proof, not another liveness query.
        // Every input must carry positive same-epoch chain evidence, name an
        // exact surviving pool output, or be released by this RBF Plan.
        for input in candidate.verified.payload().footprint.inputs() {
            if candidate.verified.payload().is_chain_input(input)
                || self
                    .membership
                    .spender(input)
                    .is_some_and(|spender| removed.contains(spender))
            {
                continue;
            }
            if self.surviving_pool_parent(input, removed)?.is_none() {
                return Err(super::PlanError::Membership(
                    MembershipReject::MissingInputEvidence(input.clone()),
                ));
            }
        }
        Ok(())
    }

    fn candidate_children(
        &self,
        candidate: &AcceptedEntry,
        removed: &HashSet<RawTxHash>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError> {
        let child_limit = self.membership_config.max_component;
        let mut children = HashSet::new();
        for child in self.accepted_children_of_candidate(candidate) {
            if removed.contains(child) || children.contains(child) {
                continue;
            }
            if children.len() == child_limit {
                return Err(super::PlanError::Membership(
                    MembershipReject::ComponentLimit {
                        limit: self.membership_config.max_component,
                    },
                ));
            }
            self.accepted_entry(child)?;
            children
                .try_reserve(1)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
            children.insert(child.clone());
        }
        Ok(children)
    }

    fn accepted_children_of_candidate<'authority>(
        &'authority self,
        candidate: &'authority AcceptedEntry,
    ) -> impl Iterator<Item = &'authority RawTxHash> + 'authority {
        candidate
            .record
            .tx
            .output_pts()
            .into_iter()
            .flat_map(|output| {
                self.membership.spender(&output).into_iter().chain(
                    self.membership
                        .dependency_readers(&output)
                        .into_iter()
                        .flatten(),
                )
            })
    }

    fn collect_surviving_ancestors(
        &self,
        parents: &HashSet<RawTxHash>,
        removed: &HashSet<RawTxHash>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError> {
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
            if ancestors.len() >= self.membership_config.max_ancestors {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            let parents = self
                .membership
                .parents(&ancestor)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            frontier.extend(parents.iter().cloned());
        }
        Ok(ancestors)
    }

    fn candidate_ancestors(
        &self,
        parents: &HashSet<RawTxHash>,
        removed: &HashSet<RawTxHash>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError> {
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
            if ancestors.len() >= self.membership_config.max_ancestors {
                return Err(super::PlanError::Membership(
                    MembershipReject::TooManyAncestors,
                ));
            }
            let grandparents = self
                .membership
                .parents(&parent)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ))?;
            frontier.extend(grandparents.iter().cloned());
        }
        Ok(ancestors)
    }
}
