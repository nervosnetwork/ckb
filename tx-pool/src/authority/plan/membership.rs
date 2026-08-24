mod eviction;
mod independent;
mod rbf;

pub(in crate::authority::plan) use independent::{
    IndependentMembershipChange, IndependentMembershipOutcome, PreparedIndependentMembership,
    prepare_independent_membership,
};

use super::TxPoolAuthority;
use crate::authority::{
    rejection::{ComponentLimitKind, MembershipReject},
    resources::AcceptedCost,
    shard::{
        AUTHORITY_SHARD_COUNT, ShardedAcceptedReadGuard, ShardedOwnerMap, ShardedOwnerWriteCut,
    },
    state::{
        AcceptedEntry, AcceptedStatus, Arrival, OwnedTx, PreAcceptedEntry, RawTxHash,
        ReplacementHistoryEntry,
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
    collections::{BinaryHeap, HashMap, HashSet, VecDeque},
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct StatusCounts {
    pub(in crate::authority) pending: usize,
    pub(in crate::authority) gap: usize,
    pub(in crate::authority) proposed: usize,
}

impl StatusCounts {
    pub(in crate::authority) fn checked_add(self, status: AcceptedStatus) -> Option<Self> {
        let mut next = self;
        let count = next.for_status_mut(status);
        *count = count.checked_add(1)?;
        Some(next)
    }

    pub(in crate::authority) fn checked_sub(self, status: AcceptedStatus) -> Option<Self> {
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

    pub(in crate::authority) fn checked_add_counts(self, other: Self) -> Option<Self> {
        Some(Self {
            pending: self.pending.checked_add(other.pending)?,
            gap: self.gap.checked_add(other.gap)?,
            proposed: self.proposed.checked_add(other.proposed)?,
        })
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
    ancestor_changes: Vec<(RawTxHash, Option<AncestorAggregate>)>,
    aggregate_changes: Vec<(RawTxHash, Option<DescendantAggregate>)>,
    accepted_order_removals: Vec<AcceptedOrderKey>,
    accepted_order_insertions: Vec<AcceptedOrderKey>,
    eviction_removals: Vec<EvictionOrderKey>,
    eviction_insertions: Vec<EvictionOrderKey>,
    status_counts: super::super::shard::ShardStatusCountPlan,
}

impl ProjectionDelta {
    #[cfg(test)]
    pub(in crate::authority) fn status_count_plan(
        &self,
    ) -> &super::super::shard::ShardStatusCountPlan {
        &self.status_counts
    }

    pub(super) fn take_status_counts(&mut self) -> super::super::shard::ShardStatusCountPlan {
        std::mem::take(&mut self.status_counts)
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

#[cfg(test)]
impl ProjectionDelta {
    pub(in crate::authority) fn extend_shard_support(
        &self,
        support: &mut super::super::shard_support::AuthorityShardSupport,
        exclusive: &mut super::super::shard_support::ExclusiveSupport,
    ) {
        for (input, _) in &self.spender_changes {
            support.insert(b"membership/spender", input);
        }
        for change in &self.dependency_changes {
            let dependency = match change {
                DependencyRelationChange::RemoveEdge(edge)
                | DependencyRelationChange::InsertEdge(edge) => &edge.dependency,
                DependencyRelationChange::InsertRow(row) => &row.dependency,
                DependencyRelationChange::RemoveRow(dependency) => dependency,
            };
            support.insert(b"membership/dependency-readers", dependency);
        }
        for change in &self.causal_changes {
            match change {
                CausalRelationChange::RemoveEdge(edge) | CausalRelationChange::InsertEdge(edge) => {
                    support.insert(b"membership/children", &edge.parent);
                    support.insert(b"membership/parents", &edge.child);
                }
                CausalRelationChange::InsertNode(node) => {
                    support.insert(b"membership/children", &node.hash);
                    support.insert(b"membership/parents", &node.hash);
                }
                CausalRelationChange::RemoveNode(hash) => {
                    support.insert(b"membership/children", hash);
                    support.insert(b"membership/parents", hash);
                }
            }
        }
        for (hash, _) in &self.ancestor_changes {
            support.insert(b"membership/ancestor", hash);
        }
        for (hash, _) in &self.aggregate_changes {
            support.insert(b"membership/descendant", hash);
        }
        for key in self
            .accepted_order_removals
            .iter()
            .chain(&self.accepted_order_insertions)
        {
            support.insert(b"membership/accepted-order", key.hash());
        }
        for key in self
            .eviction_removals
            .iter()
            .chain(&self.eviction_insertions)
        {
            support.insert(b"membership/eviction-order", &key.hash);
        }
        let _ = exclusive;
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
        self.entries.layout.shards[self.shard(b"membership/ancestor", hash)]
            .read()
            .ancestor_aggregates
            .get(hash)
            .copied()
    }

    pub(in crate::authority) fn descendant_aggregate(
        &self,
        hash: &RawTxHash,
    ) -> Option<DescendantAggregate> {
        self.entries.layout.shards[self.shard(b"membership/descendant", hash)]
            .read()
            .descendant_aggregates
            .get(hash)
            .copied()
    }

    pub(super) fn dependency_readers(&self, dependency: &OutPoint) -> Option<HashSet<RawTxHash>> {
        self.entries.layout.shards[self.shard(b"membership/dependency-readers", dependency)]
            .read()
            .dependency_readers
            .get(dependency)
            .cloned()
    }

    fn dependency_reader_row_facts(
        &self,
        dependency: &OutPoint,
        reader: &RawTxHash,
    ) -> Option<(usize, bool)> {
        let shard = self.entries.layout.shards
            [self.shard(b"membership/dependency-readers", dependency)]
        .read();
        shard
            .dependency_readers
            .get(dependency)
            .map(|readers| (readers.len(), readers.contains(reader)))
    }

    pub(in crate::authority) fn parents(&self, hash: &RawTxHash) -> Option<HashSet<RawTxHash>> {
        self.entries.layout.shards[self.shard(b"membership/parents", hash)]
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

    pub(in crate::authority) fn contains_accepted_order(&self, key: &AcceptedOrderKey) -> bool {
        self.entries.layout.shards[self.shard(b"membership/accepted-order", key.hash())]
            .read()
            .accepted_order
            .contains(key)
    }

    pub(in crate::authority) fn contains_eviction_order(&self, key: &EvictionOrderKey) -> bool {
        self.entries.layout.shards[self.shard(b"membership/eviction-order", &key.hash)]
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
        self.entries.layout.shards[self.shard(b"membership/children", hash)]
            .read()
            .children
            .get(hash)
            .cloned()
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
            for domain in [
                b"membership/parents".as_slice(),
                b"membership/children".as_slice(),
                b"membership/ancestor".as_slice(),
                b"membership/descendant".as_slice(),
            ] {
                let shard = self.shard(domain, owner);
                owner_additions[shard] =
                    owner_additions[shard]
                        .checked_add(1)
                        .ok_or(super::PlanError::Fault(
                            super::AuthorityFault::CounterExhausted,
                        ))?;
            }
        }
        for (shard, (inputs, owners)) in self
            .entries
            .layout
            .shards
            .iter()
            .zip(input_additions.into_iter().zip(owner_additions))
        {
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

    pub(super) fn reserve_dependency_reader_rows<'dependency>(
        &self,
        dependencies: impl IntoIterator<Item = &'dependency OutPoint>,
    ) -> Result<(), super::PlanError> {
        let mut additions = [0usize; AUTHORITY_SHARD_COUNT];
        for dependency in dependencies {
            let shard = self.shard(b"membership/dependency-readers", dependency);
            additions[shard] = additions[shard]
                .checked_add(1)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
        }
        for (shard, additional) in self.entries.layout.shards.iter().zip(additions) {
            if additional == 0 {
                continue;
            }
            shard
                .write()
                .dependency_readers
                .try_reserve(additional)
                .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        }
        Ok(())
    }

    pub(super) fn reserve_dependency_reader_row(
        &self,
        dependency: &OutPoint,
        additional: usize,
    ) -> Result<(), super::PlanError> {
        self.entries.layout.shards[self.shard(b"membership/dependency-readers", dependency)]
            .write()
            .dependency_readers
            .get_mut(dependency)
            .ok_or(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ))?
            .try_reserve(additional)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))
    }

    pub(super) fn reserve_child_row(
        &self,
        parent: &RawTxHash,
        additional: usize,
    ) -> Result<(), super::PlanError> {
        self.entries.layout.shards[self.shard(b"membership/children", parent)]
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
        self.entries.layout.shards[self.shard(b"membership/parents", child)]
            .write()
            .parents
            .get_mut(child)
            .ok_or(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ))?
            .try_reserve(additional)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))
    }

    pub(super) fn apply(&mut self, delta: ProjectionDelta) {
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
        for change in &self.dependency_changes {
            let dependency = match change {
                DependencyRelationChange::RemoveEdge(edge)
                | DependencyRelationChange::InsertEdge(edge) => &edge.dependency,
                DependencyRelationChange::InsertRow(row) => &row.dependency,
                DependencyRelationChange::RemoveRow(dependency) => dependency,
            };
            support.insert(
                entries
                    .layout
                    .router
                    .shard(b"membership/dependency-readers", dependency),
            );
        }
        for change in &self.causal_changes {
            match change {
                CausalRelationChange::RemoveEdge(edge) | CausalRelationChange::InsertEdge(edge) => {
                    support.insert(
                        entries
                            .layout
                            .router
                            .shard(b"membership/children", &edge.parent),
                    );
                    support.insert(
                        entries
                            .layout
                            .router
                            .shard(b"membership/parents", &edge.child),
                    );
                }
                CausalRelationChange::InsertNode(node) => {
                    support.insert(
                        entries
                            .layout
                            .router
                            .shard(b"membership/parents", &node.hash),
                    );
                    support.insert(
                        entries
                            .layout
                            .router
                            .shard(b"membership/children", &node.hash),
                    );
                }
                CausalRelationChange::RemoveNode(hash) => {
                    support.insert(entries.layout.router.shard(b"membership/parents", hash));
                    support.insert(entries.layout.router.shard(b"membership/children", hash));
                }
            }
        }
        for (hash, _) in &self.ancestor_changes {
            support.insert(entries.layout.router.shard(b"membership/ancestor", hash));
        }
        for (hash, _) in &self.aggregate_changes {
            support.insert(entries.layout.router.shard(b"membership/descendant", hash));
        }
        for key in self
            .accepted_order_removals
            .iter()
            .chain(&self.accepted_order_insertions)
        {
            support.insert(
                entries
                    .layout
                    .router
                    .shard(b"membership/accepted-order", key.hash()),
            );
        }
        for key in self
            .eviction_removals
            .iter()
            .chain(&self.eviction_insertions)
        {
            support.insert(
                entries
                    .layout
                    .router
                    .shard(b"membership/eviction-order", &key.hash),
            );
        }
        support
    }

    pub(in crate::authority) fn apply_sharded(
        self,
        entries: &ShardedOwnerMap,
        cut: &mut ShardedOwnerWriteCut<'_>,
    ) {
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
        for change in self.dependency_changes {
            match change {
                DependencyRelationChange::RemoveEdge(edge) => {
                    let shard = entries
                        .layout
                        .router
                        .shard(b"membership/dependency-readers", &edge.dependency);
                    if let Some(readers) = cut
                        .projection_shard_mut(shard)
                        .dependency_readers
                        .get_mut(&edge.dependency)
                    {
                        readers.remove(&edge.reader);
                    }
                }
                DependencyRelationChange::InsertRow(row) => {
                    let shard = entries
                        .layout
                        .router
                        .shard(b"membership/dependency-readers", &row.dependency);
                    cut.projection_shard_mut(shard)
                        .dependency_readers
                        .insert(row.dependency, row.readers);
                }
                DependencyRelationChange::InsertEdge(edge) => {
                    let shard = entries
                        .layout
                        .router
                        .shard(b"membership/dependency-readers", &edge.dependency);
                    if let Some(readers) = cut
                        .projection_shard_mut(shard)
                        .dependency_readers
                        .get_mut(&edge.dependency)
                    {
                        readers.insert(edge.reader);
                    }
                }
                DependencyRelationChange::RemoveRow(dependency) => {
                    let shard = entries
                        .layout
                        .router
                        .shard(b"membership/dependency-readers", &dependency);
                    cut.projection_shard_mut(shard)
                        .dependency_readers
                        .remove(&dependency);
                }
            }
        }
        for change in self.causal_changes {
            match change {
                CausalRelationChange::RemoveEdge(edge) => {
                    let children_shard = entries
                        .layout
                        .router
                        .shard(b"membership/children", &edge.parent);
                    if let Some(children) = cut
                        .projection_shard_mut(children_shard)
                        .children
                        .get_mut(&edge.parent)
                    {
                        children.remove(&edge.child);
                    }
                    let parents_shard = entries
                        .layout
                        .router
                        .shard(b"membership/parents", &edge.child);
                    if let Some(parents) = cut
                        .projection_shard_mut(parents_shard)
                        .parents
                        .get_mut(&edge.child)
                    {
                        parents.remove(&edge.parent);
                    }
                }
                CausalRelationChange::InsertNode(node) => {
                    let parents_shard = entries
                        .layout
                        .router
                        .shard(b"membership/parents", &node.hash);
                    cut.projection_shard_mut(parents_shard)
                        .parents
                        .insert(node.hash.clone(), node.parents);
                    let children_shard = entries
                        .layout
                        .router
                        .shard(b"membership/children", &node.hash);
                    cut.projection_shard_mut(children_shard)
                        .children
                        .insert(node.hash, node.children);
                }
                CausalRelationChange::InsertEdge(edge) => {
                    let children_shard = entries
                        .layout
                        .router
                        .shard(b"membership/children", &edge.parent);
                    if let Some(children) = cut
                        .projection_shard_mut(children_shard)
                        .children
                        .get_mut(&edge.parent)
                    {
                        children.insert(edge.child.clone());
                    }
                    let parents_shard = entries
                        .layout
                        .router
                        .shard(b"membership/parents", &edge.child);
                    if let Some(parents) = cut
                        .projection_shard_mut(parents_shard)
                        .parents
                        .get_mut(&edge.child)
                    {
                        parents.insert(edge.parent);
                    }
                }
                CausalRelationChange::RemoveNode(hash) => {
                    let parents_shard = entries.layout.router.shard(b"membership/parents", &hash);
                    cut.projection_shard_mut(parents_shard)
                        .parents
                        .remove(&hash);
                    let children_shard = entries.layout.router.shard(b"membership/children", &hash);
                    cut.projection_shard_mut(children_shard)
                        .children
                        .remove(&hash);
                }
            }
        }
        for (hash, aggregate) in self.ancestor_changes {
            let shard = entries.layout.router.shard(b"membership/ancestor", &hash);
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
            let shard = entries.layout.router.shard(b"membership/descendant", &hash);
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
            let shard = entries
                .layout
                .router
                .shard(b"membership/accepted-order", key.hash());
            cut.projection_shard_mut(shard).accepted_order.remove(&key);
        }
        for key in self.accepted_order_insertions {
            let shard = entries
                .layout
                .router
                .shard(b"membership/accepted-order", key.hash());
            cut.projection_shard_mut(shard).accepted_order.insert(key);
        }
        for key in self.eviction_removals {
            let shard = entries
                .layout
                .router
                .shard(b"membership/eviction-order", &key.hash);
            cut.projection_shard_mut(shard).eviction_order.remove(&key);
        }
        for key in self.eviction_insertions {
            let shard = entries
                .layout
                .router
                .shard(b"membership/eviction-order", &key.hash);
            cut.projection_shard_mut(shard).eviction_order.insert(key);
        }
    }
}

#[cfg(test)]
#[path = "../tests/support/plan_membership.rs"]
pub(in crate::authority) mod test_support;

impl TxPoolAuthority {
    /// Complete Accepted descendant closure for a trusted administrative
    /// removal. Unlike RBF/capacity policy, this compatibility operation is
    /// bounded by actual resident ownership and may remove a whole component.
    /// The returned order is children-first, although the total batch compiler
    /// does not depend on mutation order.
    pub(super) fn administrative_descendant_closure(
        &self,
        root: &RawTxHash,
    ) -> Result<Vec<RawTxHash>, super::PlanError> {
        if !matches!(
            self.entries.get(root).as_deref(),
            Some(OwnedTx::Accepted(_))
        ) {
            return Err(super::PlanError::Stale(super::StalePlan::Phase));
        }
        let closure = self.bounded_descendant_postorder(
            std::slice::from_ref(root),
            &HashSet::new(),
            self.entries.len(),
            ComponentLimitKind::Mutation,
        );
        match closure {
            // `remaining_limit` is the complete owner population, so a real
            // Accepted closure cannot reach the policy limit. Treat such a
            // result as a projection fault instead of a public mutation rule.
            Err(super::PlanError::Membership(_)) => Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            )),
            other => other,
        }
    }

    pub(super) fn evaluate_preaccepted_membership(
        &self,
        hash: &RawTxHash,
        before: &PreAcceptedEntry,
        candidate: &AcceptedEntry,
    ) -> Result<MembershipEvaluation, super::PlanError> {
        Self::validate_preaccepted_membership_subject(hash, before, candidate)?;
        self.evaluate_membership_candidate(hash, candidate)
    }

    pub(super) fn prepare_membership_after_evaluation(
        &mut self,
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

    /// Compile policy, eviction and accepted projections for a candidate
    /// whose immutable validation evidence has already been sealed. Source-
    /// specific owner checks stay in the command wrapper; RBF and capacity
    /// policy have exactly one implementation for asynchronous and direct
    /// admissions.
    pub(super) fn prepare_membership_candidate(
        &mut self,
        hash: &RawTxHash,
        candidate: &AcceptedEntry,
    ) -> Result<PreparedMembership, super::PlanError> {
        let evaluation = self.evaluate_membership_candidate(hash, candidate)?;
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
        let mandatory = rbf::replacement_removals(self, candidate)?;
        eviction::complete_removals(self, hash, candidate, mandatory)
    }

    fn compile_membership_evaluation(
        &mut self,
        hash: &RawTxHash,
        candidate: &AcceptedEntry,
        evaluation: MembershipEvaluation,
    ) -> Result<PreparedMembership, super::PlanError> {
        let MembershipEvaluation {
            removals: selected_removals,
            candidate_parents,
            candidate_children,
            aggregate,
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
            projection,
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
        let mut dependency_capacity = 0usize;
        for hash in removals.iter() {
            let entry = self.accepted_entry(hash)?;
            status_count_changes.push((hash.clone(), Some(entry.status()), None));
            input_capacity = input_capacity
                .checked_add(entry.proof.payload().footprint.inputs().len())
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
            dependency_capacity = dependency_capacity
                .checked_add(entry.proof.payload().footprint.dependencies().len())
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
        let status_counts = self.entries.plan_status_counts(
            status_count_changes
                .iter()
                .map(|(hash, before, after)| (hash, *before, *after)),
        )?;

        let mut spender_changes = Vec::new();
        spender_changes
            .try_reserve(input_capacity)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut dependency_removals = Vec::new();
        dependency_removals
            .try_reserve(dependency_capacity)
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
            for dependency in entry.proof.payload().footprint.dependencies() {
                if !self
                    .membership
                    .dependency_readers(dependency)
                    .is_some_and(|readers| readers.contains(hash))
                {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                dependency_removals.push(DependencyReaderEdge {
                    dependency: dependency.clone(),
                    reader: hash.clone(),
                });
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
        dependency_removals.sort_unstable();
        dependency_removals.dedup();
        causal_removals.sort_unstable();
        causal_removals.dedup();
        let (dependency_rows, dependency_row_removals) =
            self.prepare_dependency_edge_capacity(&dependency_removals, &[])?;
        if !dependency_rows.is_empty() {
            return Err(super::PlanError::Fault(
                super::AuthorityFault::MembershipProjection,
            ));
        }
        let dependency_changes = dependency_change_log(
            dependency_removals,
            Vec::new(),
            Vec::new(),
            dependency_row_removals,
        )?;
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

        Ok(ProjectionDelta {
            spender_changes,
            dependency_changes,
            causal_changes,
            ancestor_changes: ancestor.changes,
            aggregate_changes,
            accepted_order_removals: ancestor.order_removals,
            accepted_order_insertions: ancestor.order_insertions,
            eviction_removals,
            eviction_insertions,
            status_counts,
        })
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
        let status_counts = self.entries.plan_status_counts(
            status_count_changes
                .iter()
                .map(|(hash, before, after)| (hash, *before, *after)),
        )?;

        let footprint = &candidate.proof.payload().footprint;
        let mut removal_inputs = 0usize;
        let mut removal_dependencies = 0usize;
        let mut removal_causal_edges = 0usize;
        for planned in removals {
            let entry = self.accepted_entry(&planned.hash)?;
            removal_inputs = removal_inputs
                .checked_add(entry.proof.payload().footprint.inputs().len())
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
            removal_dependencies = removal_dependencies
                .checked_add(entry.proof.payload().footprint.dependencies().len())
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
            for input in entry.proof.payload().footprint.inputs() {
                if self.membership.spender(input) != Some(removal.clone()) {
                    return Err(super::PlanError::Fault(
                        super::AuthorityFault::MembershipProjection,
                    ));
                }
                spender_after.insert(input.clone(), None);
            }
            for dependency in entry.proof.payload().footprint.dependencies() {
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

        self.reserve_membership_owner_insertions(footprint.inputs().iter(), std::iter::once(hash))?;

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
            ancestor_changes: aggregate.ancestor_changes,
            aggregate_changes: aggregate.changes,
            accepted_order_removals: aggregate.accepted_order_removals,
            accepted_order_insertions: aggregate.accepted_order_insertions,
            eviction_removals: aggregate.eviction_removals,
            eviction_insertions: aggregate.eviction_insertions,
            status_counts,
        })
    }

    fn prepare_dependency_edge_capacity(
        &self,
        removals: &[DependencyReaderEdge],
        insertions: &[DependencyReaderEdge],
    ) -> Result<(Vec<PreparedDependencyRow>, Vec<OutPoint>), super::PlanError> {
        struct RowCounts {
            removals: usize,
            insertions: usize,
            existing_len: Option<usize>,
        }

        let edge_count =
            removals
                .len()
                .checked_add(insertions.len())
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
        let mut counts = HashMap::<OutPoint, RowCounts>::new();
        counts
            .try_reserve(edge_count)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        for edge in removals {
            let Some((existing_len, contains_reader)) = self
                .membership
                .dependency_reader_row_facts(&edge.dependency, &edge.reader)
            else {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            };
            if !contains_reader {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            let count = counts.entry(edge.dependency.clone()).or_insert(RowCounts {
                removals: 0,
                insertions: 0,
                existing_len: Some(existing_len),
            });
            count.removals = count
                .removals
                .checked_add(1)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
        }
        for edge in insertions {
            let row_facts = self
                .membership
                .dependency_reader_row_facts(&edge.dependency, &edge.reader);
            if row_facts.is_some_and(|(_, contains_reader)| contains_reader) {
                return Err(super::PlanError::Fault(
                    super::AuthorityFault::MembershipProjection,
                ));
            }
            let count = counts.entry(edge.dependency.clone()).or_insert(RowCounts {
                removals: 0,
                insertions: 0,
                existing_len: row_facts.map(|(existing_len, _)| existing_len),
            });
            count.insertions = count
                .insertions
                .checked_add(1)
                .ok_or(super::PlanError::Fault(
                    super::AuthorityFault::CounterExhausted,
                ))?;
        }

        let new_rows = counts
            .values()
            .filter(|count| count.existing_len.is_none())
            .count();
        self.reserve_membership_dependency_rows(
            counts.iter().filter_map(|(dependency, count)| {
                count.existing_len.is_none().then_some(dependency)
            }),
        )?;
        let mut row_insertions = Vec::new();
        row_insertions
            .try_reserve(new_rows)
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut row_removals = Vec::new();
        row_removals
            .try_reserve(counts.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut ordered_counts = Vec::new();
        ordered_counts
            .try_reserve_exact(counts.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        ordered_counts.extend(counts);
        ordered_counts.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        for (dependency, count) in ordered_counts {
            match count.existing_len {
                Some(existing_len) => {
                    let remaining =
                        existing_len
                            .checked_sub(count.removals)
                            .ok_or(super::PlanError::Fault(
                                super::AuthorityFault::MembershipProjection,
                            ))?;
                    let final_count =
                        remaining
                            .checked_add(count.insertions)
                            .ok_or(super::PlanError::Fault(
                                super::AuthorityFault::CounterExhausted,
                            ))?;
                    if count.insertions != 0 {
                        self.reserve_membership_dependency_row(&dependency, count.insertions)?;
                    }
                    if final_count == 0 {
                        row_removals.push(dependency);
                    }
                }
                None => {
                    if count.removals != 0 || count.insertions == 0 {
                        return Err(super::PlanError::Fault(
                            super::AuthorityFault::MembershipProjection,
                        ));
                    }
                    let mut readers = HashSet::new();
                    readers.try_reserve(count.insertions).map_err(|_| {
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

    fn bounded_descendant_postorder(
        &self,
        roots: &[RawTxHash],
        excluded: &HashSet<RawTxHash>,
        remaining_limit: usize,
        limit_kind: ComponentLimitKind,
    ) -> Result<Vec<RawTxHash>, super::PlanError> {
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
                if excluded.contains(&child) || closure.contains(&child) {
                    continue;
                }
                if closure.len() == remaining_limit {
                    return Err(super::PlanError::Membership(
                        MembershipReject::ComponentLimit {
                            kind: limit_kind,
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
        let mut leaves = BinaryHeap::new();
        leaves
            .try_reserve(closure.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
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
                leaves.push(Reverse(hash.clone()));
            }
        }
        let mut ordered = Vec::new();
        ordered
            .try_reserve(closure.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        while let Some(Reverse(hash)) = leaves.pop() {
            ordered.push(hash.clone());
            let parents = self
                .membership
                .parents(&hash)
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
        Ok(ordered)
    }

    fn candidate_parents(
        &self,
        candidate: &AcceptedEntry,
        removed: &HashSet<RawTxHash>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError> {
        let footprint = &candidate.proof.payload().footprint;
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
        let owner = self.entries.get(&parent);
        let Some(OwnedTx::Accepted(entry)) = owner.as_deref() else {
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
        for input in candidate.proof.payload().footprint.inputs() {
            if candidate.proof.is_chain_input(input)
                || self
                    .membership
                    .spender(input)
                    .is_some_and(|spender| removed.contains(&spender))
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

    fn validate_candidate_dependency_evidence(
        &self,
        candidate: &AcceptedEntry,
        removed: &HashSet<RawTxHash>,
    ) -> Result<(), super::PlanError> {
        // A resolved dependency is either positive chain evidence from the
        // final validation view or an exact surviving Accepted output. This
        // closes the resolver boundary: PreAccepted outputs can never become
        // causal membership merely because a future resolver exposed them.
        for dependency in candidate.proof.payload().footprint.dependencies() {
            if candidate.proof.is_chain_dependency(dependency)
                || self.surviving_pool_parent(dependency, removed)?.is_some()
            {
                continue;
            }
            return Err(super::PlanError::Membership(
                MembershipReject::MissingDependencyEvidence(dependency.clone()),
            ));
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
            if removed.contains(&child) || children.contains(&child) {
                continue;
            }
            if children.len() == child_limit {
                return Err(super::PlanError::Membership(
                    MembershipReject::ComponentLimit {
                        kind: ComponentLimitKind::Mutation,
                        limit: self.membership_config.max_component,
                    },
                ));
            }
            self.accepted_entry(&child)?;
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
    ) -> impl Iterator<Item = RawTxHash> + 'authority {
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

    /// Return every surviving ancestor that previously counted `descendant`.
    /// Unlike final-graph ancestry, aggregate subtraction must traverse
    /// through removed intermediate nodes: each removed descendant was part
    /// of every such ancestor's pre-transition aggregate.
    fn collect_surviving_ancestors_through_removals(
        &self,
        descendant: &RawTxHash,
        removed: &HashSet<RawTxHash>,
    ) -> Result<HashSet<RawTxHash>, super::PlanError> {
        let parents = self
            .membership
            .parents(descendant)
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
            if visited.len() >= self.membership_config.max_ancestors {
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
                self.membership
                    .parents(&ancestor)
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
