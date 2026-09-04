use super::super::{AuthorityFault, PlanError};
use super::*;
use std::collections::BTreeSet;

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
    pub(in crate::authority) parents: HashMap<RawTxHash, HashSet<RawTxHash>>,
    pub(in crate::authority) children: HashMap<RawTxHash, HashSet<RawTxHash>>,
    pub(in crate::authority) ancestor_aggregates: HashMap<RawTxHash, AncestorAggregate>,
    pub(in crate::authority) descendant_aggregates: HashMap<RawTxHash, DescendantAggregate>,
    pub(in crate::authority) accepted_order: BTreeSet<AcceptedOrderKey>,
    pub(in crate::authority) eviction_order: BTreeSet<EvictionOrderKey>,
    pub(in crate::authority) counts: StatusCounts,
}

impl MembershipProjection {
    pub(in crate::authority) fn snapshot(&self, counts: StatusCounts) -> MembershipSnapshot {
        let shards: [ckb_util::parking_lot::RwLockReadGuard<
            '_,
            crate::authority::shard::AuthorityShard,
        >; crate::authority::shard::AUTHORITY_SHARD_COUNT] =
            std::array::from_fn(|shard| self.entries.layout.shards[shard].read());
        let mut spenders = HashMap::new();
        let mut parents = HashMap::new();
        let mut children = HashMap::new();
        let mut ancestor_aggregates = HashMap::new();
        let mut descendant_aggregates = HashMap::new();
        let mut accepted_order = BTreeSet::new();
        let mut eviction_order = BTreeSet::new();
        for shard in &shards {
            spenders.extend(
                shard
                    .spenders
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
            parents.extend(
                shard
                    .parents
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
            children.extend(
                shard
                    .children
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
            ancestor_aggregates.extend(
                shard
                    .ancestor_aggregates
                    .iter()
                    .map(|(key, value)| (key.clone(), *value)),
            );
            descendant_aggregates.extend(
                shard
                    .descendant_aggregates
                    .iter()
                    .map(|(key, value)| (key.clone(), *value)),
            );
            accepted_order.extend(shard.accepted_order.iter().cloned());
            eviction_order.extend(shard.eviction_order.iter().cloned());
        }
        MembershipSnapshot {
            spenders,
            parents,
            children,
            ancestor_aggregates,
            descendant_aggregates,
            accepted_order,
            eviction_order,
            counts,
        }
    }

    /// Rebuild the complete membership projection from the sole Accepted
    /// owner authority and compare every stored row and ordered key.
    ///
    /// This is a test-only production refinement oracle. It owns no policy
    /// and is never available to a production read or recovery path.
    pub(in crate::authority) fn semantically_matches(
        &self,
        entries: &crate::authority::shard::ShardedOwnerMap,
    ) -> bool {
        let snapshot = entries.snapshot_for_test();
        let accepted = snapshot
            .iter()
            .filter_map(|(hash, owner)| match owner {
                OwnedTx::Accepted(entry) => Some((hash, entry)),
                OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => None,
            })
            .collect::<HashMap<_, _>>();
        let mut spenders = HashMap::new();
        let mut parents = accepted
            .keys()
            .map(|hash| ((*hash).clone(), HashSet::new()))
            .collect::<HashMap<_, _>>();
        let mut children = parents.clone();
        let mut counts = StatusCounts::default();

        for (hash, entry) in &accepted {
            let Some(next_counts) = counts.checked_add(entry.status()) else {
                return false;
            };
            counts = next_counts;
            for input in entry.proof.payload().footprint.inputs() {
                if spenders.insert(input.clone(), (*hash).clone()).is_some() {
                    return false;
                }
            }
            for out_point in entry
                .proof
                .payload()
                .footprint
                .inputs()
                .iter()
                .chain(entry.proof.payload().footprint.dependencies())
            {
                let parent = RawTxHash(out_point.tx_hash());
                if !accepted.contains_key(&parent) {
                    continue;
                }
                let Some(parent_row) = parents.get_mut(*hash) else {
                    return false;
                };
                parent_row.insert(parent.clone());
                let Some(child_row) = children.get_mut(&parent) else {
                    return false;
                };
                child_row.insert((*hash).clone());
            }
        }

        let mut ancestor_aggregates = HashMap::new();
        let mut descendant_aggregates = HashMap::new();
        let mut accepted_order = BTreeSet::new();
        let mut eviction_order = BTreeSet::new();
        for (root, root_entry) in &accepted {
            let mut ancestor_aggregate = AncestorAggregate::default();
            let mut visited = HashSet::new();
            let mut frontier = VecDeque::from([(*root).clone()]);
            while let Some(ancestor) = frontier.pop_front() {
                if !visited.insert(ancestor.clone()) {
                    continue;
                }
                let Some(entry) = accepted.get(&ancestor) else {
                    return false;
                };
                let cost = entry.proof.metrics().cost;
                let Some(entries) = ancestor_aggregate.entries.checked_add(1) else {
                    return false;
                };
                ancestor_aggregate.entries = entries;
                let Some(serialized_bytes) = ancestor_aggregate
                    .serialized_bytes
                    .checked_add(cost.serialized_bytes)
                else {
                    return false;
                };
                ancestor_aggregate.serialized_bytes = serialized_bytes;
                let Some(cycles) = ancestor_aggregate.cycles.checked_add(cost.cycles) else {
                    return false;
                };
                ancestor_aggregate.cycles = cycles;
                let Ok(fee) = ancestor_aggregate.fee.safe_add(entry.proof.metrics().fee) else {
                    return false;
                };
                ancestor_aggregate.fee = fee;
                let Some(parent_row) = parents.get(&ancestor) else {
                    return false;
                };
                frontier.extend(parent_row.iter().cloned());
            }
            ancestor_aggregates.insert((*root).clone(), ancestor_aggregate);
            accepted_order.insert(AcceptedOrderKey::new(root_entry, ancestor_aggregate));

            let mut descendant_aggregate = DescendantAggregate::default();
            let mut visited = HashSet::new();
            let mut frontier = VecDeque::from([(*root).clone()]);
            while let Some(descendant) = frontier.pop_front() {
                if !visited.insert(descendant.clone()) {
                    continue;
                }
                let Some(entry) = accepted.get(&descendant) else {
                    return false;
                };
                let cost = entry.proof.metrics().cost;
                let Some(entries) = descendant_aggregate.entries.checked_add(1) else {
                    return false;
                };
                descendant_aggregate.entries = entries;
                let Some(serialized_bytes) = descendant_aggregate
                    .serialized_bytes
                    .checked_add(cost.serialized_bytes)
                else {
                    return false;
                };
                descendant_aggregate.serialized_bytes = serialized_bytes;
                let Some(cycles) = descendant_aggregate.cycles.checked_add(cost.cycles) else {
                    return false;
                };
                descendant_aggregate.cycles = cycles;
                let Ok(fee) = descendant_aggregate.fee.safe_add(entry.proof.metrics().fee) else {
                    return false;
                };
                descendant_aggregate.fee = fee;
                let Some(child_row) = children.get(&descendant) else {
                    return false;
                };
                frontier.extend(child_row.iter().cloned());
            }
            descendant_aggregates.insert((*root).clone(), descendant_aggregate);
            let cost = root_entry.proof.metrics().cost;
            let self_rate = FeeRate::calculate(
                root_entry.proof.metrics().fee,
                get_transaction_weight(cost.serialized_bytes, cost.cycles),
            );
            let descendants_rate = FeeRate::calculate(
                descendant_aggregate.fee,
                get_transaction_weight(
                    descendant_aggregate.serialized_bytes,
                    descendant_aggregate.cycles,
                ),
            );
            eviction_order.insert(EvictionOrderKey {
                status: root_entry.status(),
                fee_rate: self_rate.max(descendants_rate),
                descendants_count: descendant_aggregate.entries,
                arrival: root_entry.record.arrival,
                hash: (*root).clone(),
            });
        }

        let Some(actual_counts) = entries.status_counts() else {
            return false;
        };
        self.snapshot(actual_counts)
            == (MembershipSnapshot {
                spenders,
                parents,
                children,
                ancestor_aggregates,
                descendant_aggregates,
                accepted_order,
                eviction_order,
                counts,
            })
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
        let proposed_counts = self.entries.plan_proposed_counts(std::iter::once((
            hash,
            Some(before.status()),
            Some(after.status()),
        )))?;
        let aggregate = self
            .membership
            .descendant_aggregate(hash)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let previous_key = EvictionOrderKey::new(before, aggregate);
        if !self.membership.contains_eviction_order(&previous_key) {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        let eviction_removals = vec![previous_key];
        let eviction_insertions = vec![EvictionOrderKey::new(after, aggregate)];
        ProjectionDelta {
            spender_changes: Vec::new(),
            causal_changes: Vec::new(),
            ancestor_changes: Vec::new(),
            aggregate_changes: Vec::new(),
            accepted_order_removals: Vec::new(),
            accepted_order_insertions: Vec::new(),
            eviction_removals,
            eviction_insertions,
            proposed_counts,
            prestate: Default::default(),
            read_witness: Default::default(),
        }
        .seal_prestate(&self.membership)
    }
}
