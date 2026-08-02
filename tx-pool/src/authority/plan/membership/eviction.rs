use super::{
    AcceptedOrderKey, AggregateDelta, AncestorAggregate, ComponentLimitKind, DescendantAggregate,
    EvictionOrderKey, MembershipEvaluation, MembershipReject, RemovalCause, SelectedRemoval,
};
use crate::authority::{
    plan::{AuthorityFault, Backpressure, PlanError, TxPoolAuthority},
    resources::AcceptedResources,
    state::{AcceptedEntry, RawTxHash},
};
use std::collections::{BTreeSet, HashMap, HashSet};

pub(super) fn complete_removals(
    authority: &TxPoolAuthority,
    candidate_hash: &RawTxHash,
    candidate: &AcceptedEntry,
    mandatory: Vec<RawTxHash>,
) -> Result<MembershipEvaluation, PlanError> {
    let candidate_fee_rate =
        EvictionOrderKey::new(candidate, DescendantAggregate::one(candidate)).fee_rate;
    let mut removed = HashSet::new();
    removed
        .try_reserve(mandatory.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    let mut removals = Vec::new();
    removals
        .try_reserve(mandatory.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    for hash in mandatory {
        if removed.insert(hash.clone()) {
            removals.push(SelectedRemoval {
                hash,
                cause: RemovalCause::Replacement,
            });
        }
    }

    authority.validate_candidate_input_evidence(candidate, &removed)?;
    authority.validate_candidate_dependency_evidence(candidate, &removed)?;
    let candidate_parents = authority.candidate_parents(candidate, &removed)?;
    let candidate_ancestors = authority.candidate_ancestors(&candidate_parents, &removed)?;
    let mut candidate_children = authority.candidate_children(candidate, &removed)?;
    // RBF victims and pre-existing descendants reached by a late parent are
    // one coupled accepted component. The candidate itself is the new owner,
    // so the configured bound counts the existing members touched around it.
    let descendant_limit = authority
        .membership_config
        .max_component
        .checked_sub(removed.len())
        .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
    let descendant_roots = candidate_children.iter().cloned().collect::<BTreeSet<_>>();
    let candidate_descendants = if descendant_roots.is_empty() {
        Vec::new()
    } else {
        authority.bounded_descendant_postorder(
            &descendant_roots,
            &removed,
            descendant_limit,
            ComponentLimitKind::Mutation,
        )?
    };
    if let Some(descendant) = candidate_descendants
        .iter()
        .filter(|descendant| candidate_ancestors.contains(*descendant))
        .min()
    {
        return Err(PlanError::Membership(MembershipReject::CausalCycle(
            descendant.clone(),
        )));
    }
    let component_capacity = removed
        .len()
        .checked_add(candidate_descendants.len())
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
    let mut component_members = HashSet::new();
    component_members
        .try_reserve(component_capacity)
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    component_members.extend(removed.iter().cloned());
    component_members.extend(candidate_descendants.iter().cloned());

    let mut projected_resources = authority.resources.accepted();
    for removal in &removals {
        let entry = authority.accepted_entry(&removal.hash)?;
        projected_resources = projected_resources
            .checked_sub(AcceptedResources::one(entry.proof.metrics().cost))
            .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
    }
    projected_resources = projected_resources
        .checked_add(AcceptedResources::one(candidate.proof.metrics().cost))
        .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;

    if removals.is_empty()
        && candidate_parents.is_empty()
        && candidate_children.is_empty()
        && authority.resources.accepted_fits(projected_resources)
    {
        let candidate_aggregate = DescendantAggregate::one(candidate);
        let candidate_ancestors = AncestorAggregate::one(candidate);
        let mut changes = Vec::new();
        changes
            .try_reserve(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        changes.push((candidate_hash.clone(), Some(candidate_aggregate)));
        let mut ancestor_changes = Vec::new();
        ancestor_changes
            .try_reserve(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        ancestor_changes.push((candidate_hash.clone(), Some(candidate_ancestors)));
        let mut accepted_order_insertions = Vec::new();
        accepted_order_insertions
            .try_reserve(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        accepted_order_insertions.push(AcceptedOrderKey::new(candidate, candidate_ancestors));
        let mut eviction_insertions = Vec::new();
        eviction_insertions
            .try_reserve(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        eviction_insertions.push(EvictionOrderKey::new(candidate, candidate_aggregate));
        return Ok(MembershipEvaluation {
            removals,
            candidate_parents,
            candidate_children,
            aggregate: AggregateDelta {
                changes,
                ancestor_changes,
                accepted_order_removals: Vec::new(),
                accepted_order_insertions,
                eviction_removals: Vec::new(),
                eviction_insertions,
            },
        });
    }

    let mut virtual_projection = VirtualProjection::new(
        candidate_hash,
        candidate,
        &candidate_ancestors,
        candidate_descendants,
    );
    virtual_projection.apply_removals(
        authority,
        removals.iter().map(|removal| &removal.hash),
        &removed,
    )?;
    virtual_projection.apply_candidate(authority, &removed)?;

    while !authority.resources.accepted_fits(projected_resources) {
        let next = virtual_projection
            .next_eviction(authority, &removed)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        if &next.hash == candidate_hash || candidate_ancestors.contains(&next.hash) {
            return Err(PlanError::Membership(MembershipReject::CandidateEvicted {
                fee_rate: candidate_fee_rate,
            }));
        }
        let roots = BTreeSet::from([next.hash]);
        let closure = authority.bounded_descendant_postorder(
            &roots,
            &removed,
            authority.membership_config.max_component,
            ComponentLimitKind::Mutation,
        )?;
        if closure
            .iter()
            .any(|hash| hash == candidate_hash || candidate_ancestors.contains(hash))
        {
            return Err(PlanError::Membership(MembershipReject::CandidateEvicted {
                fee_rate: candidate_fee_rate,
            }));
        }
        // Capacity eviction may overlap the already-proved late-descendant
        // closure. Charge only newly touched existing members, but never let
        // the union exceed the same component bound.
        let new_members = closure
            .iter()
            .filter(|hash| !component_members.contains(*hash))
            .count();
        let projected_component = component_members
            .len()
            .checked_add(new_members)
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        if projected_component > authority.membership_config.max_component {
            return Err(PlanError::Membership(MembershipReject::ComponentLimit {
                kind: ComponentLimitKind::Mutation,
                limit: authority.membership_config.max_component,
            }));
        }

        removed
            .try_reserve(closure.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        removals
            .try_reserve(closure.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        component_members
            .try_reserve(new_members)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        component_members.extend(closure.iter().cloned());
        for hash in &closure {
            if !removed.insert(hash.clone()) {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
        }
        for hash in &closure {
            let entry = authority.accepted_entry(hash)?;
            projected_resources = projected_resources
                .checked_sub(AcceptedResources::one(entry.proof.metrics().cost))
                .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
        }
        virtual_projection.remove_virtual_keys(&closure);
        virtual_projection.apply_removals(authority, closure.iter(), &removed)?;
        for hash in closure {
            removals.push(SelectedRemoval {
                hash,
                cause: RemovalCause::Capacity,
            });
        }
    }

    let aggregate = virtual_projection.finish(authority, candidate_hash, &removals)?;
    candidate_children.retain(|child| !removed.contains(child));
    Ok(MembershipEvaluation {
        removals,
        candidate_parents,
        candidate_children,
        aggregate,
    })
}

struct VirtualProjection<'candidate> {
    candidate_hash: &'candidate RawTxHash,
    candidate: &'candidate AcceptedEntry,
    candidate_ancestors: &'candidate HashSet<RawTxHash>,
    candidate_descendants: Vec<RawTxHash>,
    candidate_active: bool,
    aggregate_after: HashMap<RawTxHash, DescendantAggregate>,
    ancestor_after: HashMap<RawTxHash, AncestorAggregate>,
    virtual_keys: HashMap<RawTxHash, EvictionOrderKey>,
    virtual_order: BTreeSet<EvictionOrderKey>,
}

impl<'candidate> VirtualProjection<'candidate> {
    fn new(
        candidate_hash: &'candidate RawTxHash,
        candidate: &'candidate AcceptedEntry,
        candidate_ancestors: &'candidate HashSet<RawTxHash>,
        mut candidate_descendants: Vec<RawTxHash>,
    ) -> Self {
        candidate_descendants.sort_unstable();
        Self {
            candidate_hash,
            candidate,
            candidate_ancestors,
            candidate_descendants,
            candidate_active: false,
            aggregate_after: HashMap::new(),
            ancestor_after: HashMap::new(),
            virtual_keys: HashMap::new(),
            virtual_order: BTreeSet::new(),
        }
    }

    fn apply_removals<'a>(
        &mut self,
        authority: &TxPoolAuthority,
        removals: impl Iterator<Item = &'a RawTxHash>,
        removed_after: &HashSet<RawTxHash>,
    ) -> Result<(), PlanError> {
        for removal in removals {
            let removed_entry = authority.accepted_entry(removal)?;
            // Every removed descendant contributed once to each surviving
            // ancestor's aggregate. Walk through removed intermediate nodes
            // so a root+descendant closure subtracts the complete contribution
            // instead of stopping at the first removed parent.
            let mut ancestors =
                authority.collect_surviving_ancestors_through_removals(removal, removed_after)?;
            if self.candidate_active && self.candidate_descendants.binary_search(removal).is_ok() {
                let additional = self
                    .candidate_ancestors
                    .len()
                    .checked_add(1)
                    .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
                ancestors
                    .try_reserve(additional)
                    .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
                ancestors.insert(self.candidate_hash.clone());
                ancestors.extend(
                    self.candidate_ancestors
                        .iter()
                        .filter(|ancestor| !removed_after.contains(*ancestor))
                        .cloned(),
                );
            }
            for ancestor in ancestors {
                if removed_after.contains(&ancestor) {
                    continue;
                }
                self.ensure_current_aggregate(authority, &ancestor)?;
                let projected = self
                    .aggregate_after
                    .get_mut(&ancestor)
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                *projected = projected
                    .checked_sub_entry(removed_entry)
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                self.refresh_existing_key(authority, &ancestor)?;
            }
        }
        if self.candidate_active {
            self.candidate_descendants
                .retain(|hash| !removed_after.contains(hash));
        }
        Ok(())
    }

    fn apply_candidate(
        &mut self,
        authority: &TxPoolAuthority,
        removed: &HashSet<RawTxHash>,
    ) -> Result<(), PlanError> {
        for ancestor in self.candidate_ancestors {
            self.ensure_current_aggregate(authority, ancestor)?;
            let projected = self
                .aggregate_after
                .get_mut(ancestor)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            *projected = projected
                .checked_add_entry(self.candidate)
                .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;
        }
        for descendant in &self.candidate_descendants {
            let descendant_parents = authority
                .membership
                .parents(descendant)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let existing_ancestors =
                authority.collect_surviving_ancestors(descendant_parents, removed)?;
            let new_candidate_ancestors = self
                .candidate_ancestors
                .iter()
                .filter(|ancestor| !existing_ancestors.contains(*ancestor))
                .count();
            let projected_ancestor_count = existing_ancestors
                .len()
                .checked_add(new_candidate_ancestors)
                .and_then(|count| count.checked_add(1))
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            if projected_ancestor_count >= authority.membership_config.max_ancestors {
                return Err(PlanError::Membership(MembershipReject::TooManyAncestors));
            }
            let descendant_entry = authority.accepted_entry(descendant)?;
            let mut ancestor_after = authority
                .membership
                .ancestor_aggregates
                .get(descendant)
                .copied()
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let expected_current_count = existing_ancestors
                .len()
                .checked_add(1)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            if ancestor_after.entries != expected_current_count {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            ancestor_after = ancestor_after
                .checked_add_entry(self.candidate)
                .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;
            for ancestor in self.candidate_ancestors {
                if existing_ancestors.contains(ancestor) {
                    continue;
                }
                let projected = self
                    .aggregate_after
                    .get_mut(ancestor)
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                *projected = projected
                    .checked_add_entry(descendant_entry)
                    .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;
                ancestor_after = ancestor_after
                    .checked_add_entry(authority.accepted_entry(ancestor)?)
                    .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;
            }
            self.ancestor_after
                .try_reserve(1)
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            self.ancestor_after
                .insert(descendant.clone(), ancestor_after);
        }
        for ancestor in self.candidate_ancestors {
            self.refresh_existing_key(authority, ancestor)?;
        }
        self.aggregate_after
            .try_reserve(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        let mut aggregate = DescendantAggregate::one(self.candidate);
        for descendant in &self.candidate_descendants {
            aggregate = aggregate
                .checked_add_entry(authority.accepted_entry(descendant)?)
                .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;
        }
        self.aggregate_after
            .insert(self.candidate_hash.clone(), aggregate);
        let mut ancestor_aggregate = AncestorAggregate::one(self.candidate);
        for ancestor in self.candidate_ancestors {
            ancestor_aggregate = ancestor_aggregate
                .checked_add_entry(authority.accepted_entry(ancestor)?)
                .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;
        }
        self.ancestor_after
            .try_reserve(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        self.ancestor_after
            .insert(self.candidate_hash.clone(), ancestor_aggregate);
        self.set_key(EvictionOrderKey::new(self.candidate, aggregate))?;
        self.candidate_active = true;
        Ok(())
    }

    fn ensure_current_aggregate(
        &mut self,
        authority: &TxPoolAuthority,
        hash: &RawTxHash,
    ) -> Result<(), PlanError> {
        if self.aggregate_after.contains_key(hash) {
            return Ok(());
        }
        let aggregate = authority
            .membership
            .descendant_aggregates
            .get(hash)
            .copied()
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        self.aggregate_after
            .try_reserve(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        self.aggregate_after.insert(hash.clone(), aggregate);
        Ok(())
    }

    fn refresh_existing_key(
        &mut self,
        authority: &TxPoolAuthority,
        hash: &RawTxHash,
    ) -> Result<(), PlanError> {
        let entry = if hash == self.candidate_hash {
            self.candidate
        } else {
            authority.accepted_entry(hash)?
        };
        let aggregate = self
            .aggregate_after
            .get(hash)
            .copied()
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        self.set_key(EvictionOrderKey::new(entry, aggregate))
    }

    fn set_key(&mut self, key: EvictionOrderKey) -> Result<(), PlanError> {
        if !self.virtual_keys.contains_key(&key.hash) {
            self.virtual_keys
                .try_reserve(1)
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        }
        if let Some(previous) = self.virtual_keys.insert(key.hash.clone(), key.clone()) {
            self.virtual_order.remove(&previous);
        }
        self.virtual_order.insert(key);
        Ok(())
    }

    fn remove_virtual_keys(&mut self, removals: &[RawTxHash]) {
        for hash in removals {
            self.aggregate_after.remove(hash);
            self.ancestor_after.remove(hash);
            if let Some(key) = self.virtual_keys.remove(hash) {
                self.virtual_order.remove(&key);
            }
        }
    }

    fn next_eviction(
        &self,
        authority: &TxPoolAuthority,
        removed: &HashSet<RawTxHash>,
    ) -> Option<EvictionOrderKey> {
        let base = authority
            .membership
            .eviction_order
            .iter()
            .find(|key| {
                !removed.contains(&key.hash) && !self.aggregate_after.contains_key(&key.hash)
            })
            .cloned();
        let sparse = self.virtual_order.first().cloned();
        match (base, sparse) {
            (Some(base), Some(sparse)) => Some(base.min(sparse)),
            (Some(base), None) => Some(base),
            (None, Some(sparse)) => Some(sparse),
            (None, None) => None,
        }
    }

    fn finish(
        self,
        authority: &TxPoolAuthority,
        candidate_hash: &RawTxHash,
        removals: &[SelectedRemoval],
    ) -> Result<AggregateDelta, PlanError> {
        let change_capacity = removals
            .len()
            .checked_add(self.aggregate_after.len())
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let mut changes = Vec::new();
        changes
            .try_reserve(change_capacity)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        changes.extend(removals.iter().map(|removal| (removal.hash.clone(), None)));
        changes.extend(
            self.aggregate_after
                .iter()
                .map(|(hash, aggregate)| (hash.clone(), Some(*aggregate))),
        );
        changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));

        let ancestor_capacity = removals
            .len()
            .checked_add(self.ancestor_after.len())
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let mut ancestor_changes = Vec::new();
        ancestor_changes
            .try_reserve(ancestor_capacity)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        ancestor_changes.extend(removals.iter().map(|removal| (removal.hash.clone(), None)));
        ancestor_changes.extend(
            self.ancestor_after
                .iter()
                .map(|(hash, aggregate)| (hash.clone(), Some(*aggregate))),
        );
        ancestor_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));

        let mut accepted_order_removals = Vec::new();
        accepted_order_removals
            .try_reserve(ancestor_capacity)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for removal in removals {
            let entry = authority.accepted_entry(&removal.hash)?;
            let aggregate = authority
                .membership
                .ancestor_aggregates
                .get(&removal.hash)
                .copied()
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let key = AcceptedOrderKey::new(entry, aggregate);
            if !authority.membership.accepted_order.contains(&key) {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            accepted_order_removals.push(key);
        }
        let mut accepted_order_insertions = Vec::new();
        accepted_order_insertions
            .try_reserve(self.ancestor_after.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for (hash, aggregate) in &self.ancestor_after {
            let entry = if hash == candidate_hash {
                self.candidate
            } else {
                let entry = authority.accepted_entry(hash)?;
                let before = authority
                    .membership
                    .ancestor_aggregates
                    .get(hash)
                    .copied()
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                let key = AcceptedOrderKey::new(entry, before);
                if !authority.membership.accepted_order.contains(&key) {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                accepted_order_removals.push(key);
                entry
            };
            accepted_order_insertions.push(AcceptedOrderKey::new(entry, *aggregate));
        }
        accepted_order_removals.sort_unstable();
        accepted_order_insertions.sort_unstable();

        let mut eviction_removals = Vec::new();
        eviction_removals
            .try_reserve(change_capacity)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for removal in removals {
            let entry = authority.accepted_entry(&removal.hash)?;
            let aggregate = authority
                .membership
                .descendant_aggregates
                .get(&removal.hash)
                .copied()
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let key = EvictionOrderKey::new(entry, aggregate);
            if !authority.membership.eviction_order.contains(&key) {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            eviction_removals.push(key);
        }
        for hash in self.aggregate_after.keys() {
            if hash == candidate_hash {
                continue;
            }
            let entry = authority.accepted_entry(hash)?;
            let aggregate = authority
                .membership
                .descendant_aggregates
                .get(hash)
                .copied()
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let key = EvictionOrderKey::new(entry, aggregate);
            if !authority.membership.eviction_order.contains(&key) {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            eviction_removals.push(key);
        }
        eviction_removals.sort_unstable();

        let mut eviction_insertions = Vec::new();
        eviction_insertions
            .try_reserve(self.virtual_order.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        eviction_insertions.extend(self.virtual_order);
        Ok(AggregateDelta {
            changes,
            ancestor_changes,
            accepted_order_removals,
            accepted_order_insertions,
            eviction_removals,
            eviction_insertions,
        })
    }
}
