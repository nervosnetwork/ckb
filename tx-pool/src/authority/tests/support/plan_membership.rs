use super::super::{AuthorityFault, Backpressure, PlanError};
use super::*;

impl MembershipConfig {
    pub(in crate::authority) fn testing_default() -> Self {
        Self {
            max_ancestors: 125,
            max_component: crate::constants::MAX_POOL_MUTATION_CANDIDATES,
            replacement: ReplacementPolicy::Disabled,
        }
    }

    pub(in crate::authority) fn testing_with_replacement(minimum_rate: FeeRate) -> Self {
        Self::from_runtime(
            125,
            crate::constants::MAX_POOL_MUTATION_CANDIDATES,
            Some(minimum_rate),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct MembershipSnapshot {
    pub(in crate::authority) spenders: HashMap<OutPoint, RawTxHash>,
    pub(in crate::authority) dependency_readers: HashMap<OutPoint, HashSet<RawTxHash>>,
    pub(in crate::authority) parents: HashMap<RawTxHash, HashSet<RawTxHash>>,
    pub(in crate::authority) children: HashMap<RawTxHash, HashSet<RawTxHash>>,
    pub(in crate::authority) ancestor_aggregates: HashMap<RawTxHash, AncestorAggregate>,
    pub(in crate::authority) descendant_aggregates: HashMap<RawTxHash, DescendantAggregate>,
    pub(in crate::authority) accepted_order: BTreeSet<AcceptedOrderKey>,
    pub(in crate::authority) eviction_order: BTreeSet<EvictionOrderKey>,
    pub(in crate::authority) counts: StatusCounts,
}

impl MembershipProjection {
    pub(in crate::authority) fn snapshot(&self) -> MembershipSnapshot {
        MembershipSnapshot {
            spenders: self.spenders.clone(),
            dependency_readers: self.dependency_readers.clone(),
            parents: self.parents.clone(),
            children: self.children.clone(),
            ancestor_aggregates: self.ancestor_aggregates.clone(),
            descendant_aggregates: self.descendant_aggregates.clone(),
            accepted_order: self.accepted_order.clone(),
            eviction_order: self.eviction_order.clone(),
            counts: self.counts,
        }
    }
}

impl TxPoolAuthority {
    pub(in crate::authority::plan) fn prepare_status_change(
        &self,
        hash: &RawTxHash,
        before: &AcceptedEntry,
        after: &AcceptedEntry,
    ) -> Result<ProjectionDelta, PlanError> {
        if before.record.identity.raw != *hash
            || after.record.identity.raw != *hash
            || before.status() == after.status()
        {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        let counts = self
            .membership
            .counts
            .checked_sub(before.status())
            .and_then(|counts| counts.checked_add(after.status()))
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let aggregate = self
            .membership
            .descendant_aggregates
            .get(hash)
            .copied()
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let previous_key = EvictionOrderKey::new(before, aggregate);
        if !self.membership.eviction_order.contains(&previous_key) {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        let mut eviction_removals = Vec::new();
        eviction_removals
            .try_reserve(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        eviction_removals.push(previous_key);
        let mut eviction_insertions = Vec::new();
        eviction_insertions
            .try_reserve(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        eviction_insertions.push(EvictionOrderKey::new(after, aggregate));
        Ok(ProjectionDelta {
            spender_changes: Vec::new(),
            dependency_changes: Vec::new(),
            causal_changes: Vec::new(),
            ancestor_changes: Vec::new(),
            aggregate_changes: Vec::new(),
            accepted_order_removals: Vec::new(),
            accepted_order_insertions: Vec::new(),
            eviction_removals,
            eviction_insertions,
            counts,
        })
    }
}
