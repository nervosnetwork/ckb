use super::{
    AcceptedOrderKey, AggregateDelta, AncestorAggregate, ComponentLimitKind, DescendantAggregate,
    EvictionOrderKey, MembershipEvaluation, MembershipReject, MembershipRemoval, PolicyContext,
    PolicyMode, RemovalCause, RemovalSelection,
};
use crate::authority::{
    plan::{AuthorityFault, PlanError},
    resources::AcceptedResources,
    state::{AcceptedEntry, RawTxHash},
};
use std::collections::{BTreeSet, HashMap, HashSet};

pub(super) fn pure_leaf_evaluation(
    candidate_hash: &RawTxHash,
    candidate: &AcceptedEntry,
) -> MembershipEvaluation {
    let candidate_aggregate = DescendantAggregate::one(candidate);
    let candidate_ancestors = AncestorAggregate::one(candidate);
    let changes = vec![(candidate_hash.clone(), Some(candidate_aggregate))];
    let ancestor_changes = vec![(candidate_hash.clone(), Some(candidate_ancestors))];
    let accepted_order_insertions = vec![AcceptedOrderKey::new(candidate, candidate_ancestors)];
    let eviction_insertions = vec![EvictionOrderKey::new(candidate, candidate_aggregate)];
    MembershipEvaluation {
        removals: Vec::new(),
        candidate_parents: HashSet::new(),
        candidate_children: HashSet::new(),
        aggregate: AggregateDelta {
            changes,
            ancestor_changes,
            accepted_order_removals: Vec::new(),
            accepted_order_insertions,
            eviction_removals: Vec::new(),
            eviction_insertions,
        },
        policy_witness: Default::default(),
    }
}

pub(super) fn complete_removals<Mode>(
    candidate_hash: &RawTxHash,
    candidate: &AcceptedEntry,
    mandatory: Vec<RawTxHash>,
    reader: &mut PolicyContext<'_, Mode>,
) -> Result<MembershipEvaluation, PlanError>
where
    Mode: PolicyMode,
{
    let config = reader.config();
    let candidate_fee_rate =
        EvictionOrderKey::new(candidate, DescendantAggregate::one(candidate)).fee_rate;
    let mut removed = HashSet::with_capacity(mandatory.len());
    let mut removals = Vec::with_capacity(mandatory.len());
    for hash in mandatory {
        if removed.insert(hash.clone()) {
            removals.push(RemovalSelection {
                hash,
                cause: RemovalCause::Replacement,
            });
        }
    }

    super::TxPoolAuthority::validate_candidate_input_evidence(candidate, &removed, reader)?;
    super::TxPoolAuthority::validate_candidate_dependency_evidence(candidate, &removed, reader)?;
    let candidate_parents = super::TxPoolAuthority::candidate_parents(candidate, &removed, reader)?;
    let candidate_ancestors =
        super::TxPoolAuthority::candidate_ancestors(&candidate_parents, &removed, reader)?;
    let mut candidate_children =
        super::TxPoolAuthority::candidate_children(candidate, &removed, reader)?;
    // RBF victims and pre-existing descendants reached by a late parent are
    // one coupled accepted component. The candidate itself is the new owner,
    // so the configured bound counts the existing members touched around it.
    let descendant_limit = config
        .max_component
        .checked_sub(removed.len())
        .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
    let mut descendant_roots = Vec::with_capacity(candidate_children.len());
    descendant_roots.extend(candidate_children.iter().cloned());
    descendant_roots.sort_unstable();
    let candidate_descendants = if descendant_roots.is_empty() {
        Vec::new()
    } else {
        super::TxPoolAuthority::bounded_descendant_postorder_with_reader(
            &descendant_roots,
            &removed,
            descendant_limit,
            ComponentLimitKind::Mutation,
            reader,
        )?
        .ordered
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
    let mut component_members = HashSet::with_capacity(component_capacity);
    component_members.extend(removed.iter().cloned());
    component_members.extend(candidate_descendants.iter().cloned());

    let mut released_resources = AcceptedResources::default();
    for removal in &removals {
        let entry = reader.observe_accepted_owner(&removal.hash)?;
        released_resources = released_resources
            .checked_add(AcceptedResources::one(entry.proof.metrics().cost))
            .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
    }
    let candidate_resources = AcceptedResources::one(candidate.proof.metrics().cost);
    // The reader capability owns the distinction between the exclusive exact
    // aggregate and the optimistic reservation-bank probe. Policy code cannot
    // reach either raw resource authority directly.
    let accepted_fits = reader.initial_accepted_fits(released_resources, candidate_resources)?;

    if removals.is_empty()
        && candidate_parents.is_empty()
        && candidate_children.is_empty()
        && accepted_fits
    {
        return Ok(pure_leaf_evaluation(candidate_hash, candidate));
    }

    let mut virtual_projection = VirtualProjection::new(
        candidate_hash,
        candidate,
        &candidate_ancestors,
        candidate_descendants,
    );
    virtual_projection.apply_removals(
        reader,
        removals.iter().map(|removal| &removal.hash),
        &removed,
    )?;
    virtual_projection.apply_candidate(&removed, reader)?;

    if !accepted_fits {
        // The capacity bank is conservative while a concurrent release crosses
        // its owner cut. Recheck the exact owner total before selecting victims;
        // only observing the eviction order widens the final witness to all
        // shards. The later bank reservation remains the capacity linearization.
        let mut projected_resources =
            reader.exact_accepted_projection(released_resources, candidate_resources)?;
        let eviction_order = if reader.accepted_fits(projected_resources) {
            Vec::new()
        } else {
            reader.eviction_order()?
        };
        let mut eviction_cursor = 0usize;
        while !reader.accepted_fits(projected_resources) {
            let next = virtual_projection
                .next_eviction(&eviction_order, &mut eviction_cursor, &removed)?
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            if &next.hash == candidate_hash || candidate_ancestors.contains(&next.hash) {
                return Err(PlanError::Membership(MembershipReject::CandidateEvicted {
                    fee_rate: candidate_fee_rate,
                }));
            }
            let closure = super::TxPoolAuthority::bounded_descendant_postorder_with_reader(
                std::slice::from_ref(&next.hash),
                &removed,
                config.max_component,
                ComponentLimitKind::Mutation,
                reader,
            )?
            .ordered;
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
            if projected_component > config.max_component {
                return Err(PlanError::Membership(MembershipReject::ComponentLimit {
                    kind: ComponentLimitKind::Mutation,
                    limit: config.max_component,
                }));
            }

            removed.reserve(closure.len());
            removals.reserve(closure.len());
            component_members.reserve(new_members);
            component_members.extend(closure.iter().cloned());
            for hash in &closure {
                if !removed.insert(hash.clone()) {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
            }
            for hash in &closure {
                let entry = reader.observe_accepted_owner(hash)?;
                projected_resources = projected_resources
                    .checked_sub(AcceptedResources::one(entry.proof.metrics().cost))
                    .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
            }
            virtual_projection.remove_virtual_keys(&closure);
            virtual_projection.apply_removals(reader, closure.iter(), &removed)?;
            for hash in closure {
                removals.push(RemovalSelection {
                    hash,
                    cause: RemovalCause::Capacity,
                });
            }
        }
    }

    let aggregate = virtual_projection.finish(reader, candidate_hash, &removals)?;
    candidate_children.retain(|child| !removed.contains(child));
    let mut captured = Vec::with_capacity(removals.len());
    for removal in removals {
        let before = reader.observe_accepted_owner(&removal.hash)?;
        captured.push(MembershipRemoval::terminal(
            (*before).clone(),
            removal.cause,
        ));
    }
    Ok(MembershipEvaluation {
        removals: captured,
        candidate_parents,
        candidate_children,
        aggregate,
        policy_witness: Default::default(),
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

    fn apply_removals<'a, Mode>(
        &mut self,
        reader: &mut PolicyContext<'_, Mode>,
        removals: impl Iterator<Item = &'a RawTxHash>,
        removed_after: &HashSet<RawTxHash>,
    ) -> Result<(), PlanError>
    where
        Mode: PolicyMode,
    {
        for removal in removals {
            let removed_entry = reader.observe_accepted_owner(removal)?;
            // Every removed descendant contributed once to each surviving
            // ancestor's aggregate. Walk through removed intermediate nodes
            // so a root+descendant closure subtracts the complete contribution
            // instead of stopping at the first removed parent.
            let mut ancestors =
                super::TxPoolAuthority::collect_surviving_ancestors_through_removals_with_reader(
                    removal,
                    removed_after,
                    reader,
                )?;
            if self.candidate_active && self.candidate_descendants.binary_search(removal).is_ok() {
                let additional = self
                    .candidate_ancestors
                    .len()
                    .checked_add(1)
                    .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
                ancestors.reserve(additional);
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
                self.ensure_current_aggregate(&ancestor, reader)?;
                let projected = self
                    .aggregate_after
                    .get_mut(&ancestor)
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                *projected = projected
                    .checked_sub_entry(&removed_entry)
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                self.refresh_existing_key(&ancestor, reader)?;
            }
        }
        if self.candidate_active {
            self.candidate_descendants
                .retain(|hash| !removed_after.contains(hash));
        }
        Ok(())
    }

    fn apply_candidate<Mode>(
        &mut self,
        removed: &HashSet<RawTxHash>,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<(), PlanError>
    where
        Mode: PolicyMode,
    {
        let max_ancestors = reader.config().max_ancestors;
        for ancestor in self.candidate_ancestors {
            self.ensure_current_aggregate(ancestor, reader)?;
            let projected = self
                .aggregate_after
                .get_mut(ancestor)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            *projected = projected
                .checked_add_entry(self.candidate)
                .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;
        }
        for descendant in &self.candidate_descendants {
            let descendant_parents = reader
                .observe_parents(descendant)?
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let existing_ancestors =
                super::TxPoolAuthority::collect_surviving_ancestors_with_reader(
                    &descendant_parents,
                    removed,
                    reader,
                )?;
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
            if projected_ancestor_count >= max_ancestors {
                return Err(PlanError::Membership(MembershipReject::TooManyAncestors));
            }
            let descendant_entry = reader.observe_accepted_owner(descendant)?;
            let mut ancestor_after = reader
                .observe_ancestor(descendant)?
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
                    .checked_add_entry(&descendant_entry)
                    .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;
                let ancestor_entry = reader.observe_accepted_owner(ancestor)?;
                ancestor_after = ancestor_after
                    .checked_add_entry(&ancestor_entry)
                    .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;
            }
            self.ancestor_after.reserve(1);
            self.ancestor_after
                .insert(descendant.clone(), ancestor_after);
        }
        for ancestor in self.candidate_ancestors {
            self.refresh_existing_key(ancestor, reader)?;
        }
        self.aggregate_after.reserve(1);
        let mut aggregate = DescendantAggregate::one(self.candidate);
        for descendant in &self.candidate_descendants {
            let descendant_entry = reader.observe_accepted_owner(descendant)?;
            aggregate = aggregate
                .checked_add_entry(&descendant_entry)
                .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;
        }
        self.aggregate_after
            .insert(self.candidate_hash.clone(), aggregate);
        let mut ancestor_aggregate = AncestorAggregate::one(self.candidate);
        for ancestor in self.candidate_ancestors {
            let ancestor_entry = reader.observe_accepted_owner(ancestor)?;
            ancestor_aggregate = ancestor_aggregate
                .checked_add_entry(&ancestor_entry)
                .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;
        }
        self.ancestor_after.reserve(1);
        self.ancestor_after
            .insert(self.candidate_hash.clone(), ancestor_aggregate);
        self.set_key(EvictionOrderKey::new(self.candidate, aggregate));
        self.candidate_active = true;
        Ok(())
    }

    fn ensure_current_aggregate<Mode>(
        &mut self,
        hash: &RawTxHash,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<(), PlanError>
    where
        Mode: PolicyMode,
    {
        if self.aggregate_after.contains_key(hash) {
            return Ok(());
        }
        let aggregate = reader
            .observe_descendant(hash)?
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        self.aggregate_after.reserve(1);
        self.aggregate_after.insert(hash.clone(), aggregate);
        Ok(())
    }

    fn refresh_existing_key<Mode>(
        &mut self,
        hash: &RawTxHash,
        reader: &mut PolicyContext<'_, Mode>,
    ) -> Result<(), PlanError>
    where
        Mode: PolicyMode,
    {
        let aggregate = self
            .aggregate_after
            .get(hash)
            .copied()
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let key = if hash == self.candidate_hash {
            EvictionOrderKey::new(self.candidate, aggregate)
        } else {
            let entry = reader.observe_accepted_owner(hash)?;
            EvictionOrderKey::new(&entry, aggregate)
        };
        self.set_key(key);
        Ok(())
    }

    fn set_key(&mut self, key: EvictionOrderKey) {
        if !self.virtual_keys.contains_key(&key.hash) {
            self.virtual_keys.reserve(1);
        }
        if let Some(previous) = self.virtual_keys.insert(key.hash.clone(), key.clone()) {
            self.virtual_order.remove(&previous);
        }
        self.virtual_order.insert(key);
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
        base_order: &[EvictionOrderKey],
        base_cursor: &mut usize,
        removed: &HashSet<RawTxHash>,
    ) -> Result<Option<EvictionOrderKey>, PlanError> {
        while base_order.get(*base_cursor).is_some_and(|key| {
            removed.contains(&key.hash) || self.aggregate_after.contains_key(&key.hash)
        }) {
            *base_cursor = base_cursor
                .checked_add(1)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        }
        let base = base_order.get(*base_cursor).cloned();
        let sparse = self.virtual_order.first().cloned();
        Ok(match (base, sparse) {
            (Some(base), Some(sparse)) if base <= sparse => {
                *base_cursor = base_cursor
                    .checked_add(1)
                    .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
                Some(base)
            }
            (Some(_), Some(sparse)) => Some(sparse),
            (Some(base), None) => {
                *base_cursor = base_cursor
                    .checked_add(1)
                    .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
                Some(base)
            }
            (None, Some(sparse)) => Some(sparse),
            (None, None) => None,
        })
    }

    fn finish<Mode>(
        self,
        reader: &mut PolicyContext<'_, Mode>,
        candidate_hash: &RawTxHash,
        removals: &[RemovalSelection],
    ) -> Result<AggregateDelta, PlanError>
    where
        Mode: PolicyMode,
    {
        let change_capacity = removals
            .len()
            .checked_add(self.aggregate_after.len())
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let mut changes = Vec::with_capacity(change_capacity);
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
        let mut ancestor_changes = Vec::with_capacity(ancestor_capacity);
        ancestor_changes.extend(removals.iter().map(|removal| (removal.hash.clone(), None)));
        ancestor_changes.extend(
            self.ancestor_after
                .iter()
                .map(|(hash, aggregate)| (hash.clone(), Some(*aggregate))),
        );
        ancestor_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));

        let mut accepted_order_removals = Vec::with_capacity(ancestor_capacity);
        for removal in removals {
            let entry = reader.observe_accepted_owner(&removal.hash)?;
            let aggregate = reader
                .observe_ancestor(&removal.hash)?
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let key = AcceptedOrderKey::new(&entry, aggregate);
            if !reader.observe_accepted_order(&key)? {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            accepted_order_removals.push(key);
        }
        let mut accepted_order_insertions = Vec::with_capacity(self.ancestor_after.len());
        for (hash, aggregate) in &self.ancestor_after {
            let key = if hash == candidate_hash {
                AcceptedOrderKey::new(self.candidate, *aggregate)
            } else {
                let entry = reader.observe_accepted_owner(hash)?;
                let before = reader
                    .observe_ancestor(hash)?
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                let key = AcceptedOrderKey::new(&entry, before);
                if !reader.observe_accepted_order(&key)? {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                accepted_order_removals.push(key);
                AcceptedOrderKey::new(&entry, *aggregate)
            };
            accepted_order_insertions.push(key);
        }
        accepted_order_removals.sort_unstable();
        accepted_order_insertions.sort_unstable();

        let mut eviction_removals = Vec::with_capacity(change_capacity);
        for removal in removals {
            let entry = reader.observe_accepted_owner(&removal.hash)?;
            let aggregate = reader
                .observe_descendant(&removal.hash)?
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let key = EvictionOrderKey::new(&entry, aggregate);
            if !reader.observe_eviction_order(&key)? {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            eviction_removals.push(key);
        }
        for hash in self.aggregate_after.keys() {
            if hash == candidate_hash {
                continue;
            }
            let entry = reader.observe_accepted_owner(hash)?;
            let aggregate = reader
                .observe_descendant(hash)?
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let key = EvictionOrderKey::new(&entry, aggregate);
            if !reader.observe_eviction_order(&key)? {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            eviction_removals.push(key);
        }
        eviction_removals.sort_unstable();

        let mut eviction_insertions = Vec::with_capacity(self.virtual_order.len());
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
