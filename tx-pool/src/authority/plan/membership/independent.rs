use super::{
    AcceptedOrderKey, AncestorAggregate, DependencyReaderEdge, DescendantAggregate,
    EvictionOrderKey, MembershipEvaluation, MembershipRemoval, PreparedCausalNode, ProjectionDelta,
    RemovalCause, causal_change_log, dependency_change_log,
};
use crate::authority::{
    plan::{AuthorityFault, Backpressure, PlanError, TxPoolAuthority},
    resources::{ResourceBatchPlan, ResourceError},
    state::{AcceptedEntry, DependencyKey, OwnedTx, PreAcceptedEntry, RawTxHash},
};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub(in crate::authority::plan) struct IndependentMembershipChange {
    pub(in crate::authority::plan) key: RawTxHash,
    pub(in crate::authority::plan) before: PreAcceptedEntry,
    pub(in crate::authority::plan) after: AcceptedEntry,
}

pub(in crate::authority::plan) struct PreparedIndependentMembership {
    pub(in crate::authority::plan) resource: ResourceBatchPlan,
    pub(in crate::authority::plan) projection: ProjectionDelta,
    pub(in crate::authority::plan) removals: Vec<Vec<MembershipRemoval>>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the prepared variant owns already-fallible Plan storage; boxing would add an independent hot-path allocation"
)]
pub(in crate::authority::plan) enum IndependentMembershipOutcome {
    Prepared(PreparedIndependentMembership),
    Coupled,
}

fn reserved_vec<T>(capacity: usize) -> Result<Vec<T>, PlanError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    Ok(values)
}

fn checked_accumulate(total: &mut usize, additional: usize) -> Result<(), PlanError> {
    *total = total
        .checked_add(additional)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
    Ok(())
}

pub(in crate::authority::plan) fn prepare_independent_membership(
    authority: &mut TxPoolAuthority,
    changes: &[IndependentMembershipChange],
) -> Result<IndependentMembershipOutcome, PlanError> {
    if changes.is_empty() {
        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
    }
    if !has_membership_relation_coupling(authority, changes)? {
        let resource = match prepare_ordinary_resources(authority, changes)? {
            Some(resource) => resource,
            None => return Ok(IndependentMembershipOutcome::Coupled),
        };
        let projection = prepare_ordinary_projection(authority, changes)?;
        return Ok(IndependentMembershipOutcome::Prepared(
            PreparedIndependentMembership {
                resource,
                projection,
                // Empty is the allocation-free ordinary representation;
                // composite membership owns exactly one row per change.
                removals: Vec::new(),
            },
        ));
    }

    let prepared = (|| {
        // Reject the common Accepted-parent/dependency cohort before running
        // the complete RBF/CPFP/capacity evaluator for every Ready member.
        // This gate is only a necessary shape check; the canonical evaluator
        // and the complete commutation proof below remain authoritative.
        let Some(victims) = possible_leaf_rbf_victims(authority, changes)? else {
            return Ok(None);
        };

        // RBF, CPFP, capacity and evidence policy run exactly once per member
        // in the canonical evaluator. A weaker member's policy rejection
        // returns to the existing strongest-head coupled route instead of
        // rejecting the whole Ready cut.
        let mut evaluations = reserved_vec(changes.len())?;
        for change in changes {
            match authority.evaluate_membership_candidate(&change.key, &change.after) {
                Ok(evaluation) => evaluations.push(evaluation),
                Err(PlanError::Membership(_)) => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        if !evaluations
            .iter()
            .zip(&victims)
            .all(|(evaluation, victim)| {
                matches!(
                    evaluation.removals.as_slice(),
                    [removal]
                        if removal.cause == RemovalCause::Replacement
                            && removal.hash == *victim
                ) && evaluation.candidate_parents.is_empty()
                    && evaluation.candidate_children.is_empty()
            })
            || !leaf_rbf_components_commute(authority, changes, &evaluations, &victims)?
        {
            return Ok(None);
        }

        let mut removals = prepare_removals(&evaluations)?;
        let Some(resource) = prepare_composite_resources(authority, changes, &mut removals)? else {
            return Ok(None);
        };
        let projection = prepare_composite_projection(authority, changes, evaluations)?;
        Ok(Some(PreparedIndependentMembership {
            resource,
            projection,
            removals,
        }))
    })();
    match prepared {
        Ok(Some(prepared)) => Ok(IndependentMembershipOutcome::Prepared(prepared)),
        Ok(None) | Err(PlanError::Backpressure(Backpressure::Allocation)) => {
            Ok(IndependentMembershipOutcome::Coupled)
        }
        Err(error) => Err(error),
    }
}

/// Conservative read-only classifier retained for coupled continuation. The
/// first-pass batch compiler separately calls the canonical evaluator before
/// admitting its narrower multi-member leaf-RBF shape.
pub(in crate::authority::plan) fn has_membership_relation_coupling(
    authority: &TxPoolAuthority,
    changes: &[IndependentMembershipChange],
) -> Result<bool, PlanError> {
    for change in changes {
        validate_owner(authority, change)?;
    }
    Ok(has_candidate_pair_coupling(changes)?
        || has_non_replacement_relation_coupling(authority, changes))
}

fn has_candidate_pair_coupling(changes: &[IndependentMembershipChange]) -> Result<bool, PlanError> {
    for (index, change) in changes.iter().enumerate() {
        // `index` comes from this exact slice. Skipping the indexed element
        // separately avoids arithmetic or saturation in the pairwise proof.
        for other in changes.iter().skip(index).skip(1) {
            if change.after.record.identity.proposal == other.after.record.identity.proposal {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            if has_shared_input(change, other) || has_conditional_edge(change, other) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn has_non_replacement_relation_coupling(
    authority: &TxPoolAuthority,
    changes: &[IndependentMembershipChange],
) -> bool {
    for (index, change) in changes.iter().enumerate() {
        let footprint = &change.after.proof.payload().footprint;

        for input in footprint.inputs() {
            if authority.membership.spender(input).is_some() {
                return true;
            }
            if authority
                .membership
                .dependency_reader_row_len(input)
                .is_some_and(|reader_count| reader_count != 0)
            {
                return true;
            }
            let producer = RawTxHash(input.tx_hash());
            if is_accepted(authority, &producer) {
                return true;
            }
            if changes
                .iter()
                .enumerate()
                .any(|(other, candidate)| other != index && candidate.key == producer)
            {
                return true;
            }
            if !change.after.proof.is_chain_input(input) {
                return true;
            }
        }

        for dependency in footprint.dependencies() {
            if authority.membership.spender(dependency).is_some() {
                return true;
            }
            let producer = RawTxHash(dependency.tx_hash());
            if is_accepted(authority, &producer) {
                return true;
            }
            if changes
                .iter()
                .enumerate()
                .any(|(other, candidate)| other != index && candidate.key == producer)
            {
                return true;
            }
            if !change.after.proof.is_chain_dependency(dependency) {
                return true;
            }
        }

        if has_accepted_child(authority, change) {
            return true;
        }
    }
    false
}

fn validate_owner(
    authority: &TxPoolAuthority,
    change: &IndependentMembershipChange,
) -> Result<(), PlanError> {
    let before = &change.before;
    let after = &change.after;
    if before.record.identity.raw != change.key
        || after.record.identity != before.record.identity
        || after.provenance != before.source.accepted_provenance()
        || after.record.arrival != before.record.arrival
        || after.proof.chain_revision() != authority.chain_revision()
    {
        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
    }
    let current = authority.entries.get(&change.key);
    match current.as_deref() {
        Some(OwnedTx::PreAccepted(current))
            if current.record.version == before.record.version
                && current.record.identity == before.record.identity => {}
        Some(OwnedTx::PreAccepted(_))
        | Some(OwnedTx::Accepted(_))
        | Some(OwnedTx::ReplacementHistory(_))
        | None => {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
    }
    if authority
        .indexes
        .proposal_owner(&before.record.identity.proposal)
        .as_ref()
        != Some(&change.key)
    {
        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
    }
    Ok(())
}

fn is_accepted(authority: &TxPoolAuthority, hash: &RawTxHash) -> bool {
    authority
        .entries
        .get(hash)
        .as_deref()
        .is_some_and(|entry| matches!(entry, OwnedTx::Accepted(_)))
}

fn has_accepted_child(authority: &TxPoolAuthority, change: &IndependentMembershipChange) -> bool {
    authority
        .accepted_children_of_candidate(&change.after)
        .next()
        .is_some()
}

fn has_shared_input(
    left: &IndependentMembershipChange,
    right: &IndependentMembershipChange,
) -> bool {
    let right_inputs = right.after.proof.payload().footprint.inputs();
    left.after
        .proof
        .payload()
        .footprint
        .inputs()
        .iter()
        .any(|input| right_inputs.binary_search(input).is_ok())
}

fn has_conditional_edge(
    left: &IndependentMembershipChange,
    right: &IndependentMembershipChange,
) -> bool {
    let left_footprint = &left.after.proof.payload().footprint;
    let right_footprint = &right.after.proof.payload().footprint;
    left_footprint
        .inputs()
        .iter()
        .any(|input| right_footprint.dependencies().binary_search(input).is_ok())
        || right_footprint
            .inputs()
            .iter()
            .any(|input| left_footprint.dependencies().binary_search(input).is_ok())
}

struct LogicalLeafFootprint {
    owners: HashSet<RawTxHash>,
    dependencies: HashSet<DependencyKey>,
    dependency_events: HashSet<DependencyKey>,
}

/// Cheap necessary gate for the only composite membership shape. It prevents
/// ordinary Accepted-parent/dependency cohorts from paying for a complete
/// per-member policy evaluation which can only fall back. Returning a victim
/// identity here grants no policy authority: the canonical evaluator must
/// independently select that exact one-victim replacement below.
fn possible_leaf_rbf_victims(
    authority: &TxPoolAuthority,
    changes: &[IndependentMembershipChange],
) -> Result<Option<Vec<RawTxHash>>, PlanError> {
    if changes.len() < 2 {
        return Ok(None);
    }
    let mut victims = reserved_vec(changes.len())?;
    for change in changes {
        let candidate_footprint = &change.after.proof.payload().footprint;
        let [candidate_input] = candidate_footprint.inputs() else {
            return Ok(None);
        };
        if !change.after.proof.is_chain_input(candidate_input)
            || candidate_footprint
                .dependencies()
                .iter()
                .any(|dependency| !change.after.proof.is_chain_dependency(dependency))
            || has_accepted_child(authority, change)
        {
            return Ok(None);
        }
        let Some(victim_hash) = authority.membership.spender(candidate_input) else {
            return Ok(None);
        };
        let victim_owner = authority
            .entries
            .get(&victim_hash)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let OwnedTx::Accepted(victim) = &*victim_owner else {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        };
        let [victim_input] = victim.proof.payload().footprint.inputs() else {
            return Ok(None);
        };
        if victim_input != candidate_input
            || !victim.proof.is_chain_input(victim_input)
            || victim
                .proof
                .payload()
                .footprint
                .dependencies()
                .iter()
                .any(|dependency| !victim.proof.is_chain_dependency(dependency))
        {
            return Ok(None);
        }
        let victim_parents = authority
            .membership
            .parents(&victim_hash)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let victim_children = authority
            .membership
            .children(&victim_hash)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        if !victim_parents.is_empty() || !victim_children.is_empty() {
            return Ok(None);
        }
        victims.push(victim_hash);
    }
    Ok(Some(victims))
}

/// Admit only the benchmark-common leaf shape. Candidate and victim each
/// have one identical chain input and no Accepted parent/child. The complete
/// logical dependency footprint includes inputs, expanded cell deps and
/// headers for candidate, victim and optional history. Components may share a
/// chain-backed read-only dependency, but every availability/loss key emitted
/// by either owner is disjoint from every other component's retained
/// dependencies, so one victim cannot stale a weaker Ready proof in the same
/// Apply. Physical shard separation is deliberately irrelevant.
fn leaf_rbf_components_commute(
    authority: &TxPoolAuthority,
    changes: &[IndependentMembershipChange],
    evaluations: &[MembershipEvaluation],
    victims: &[RawTxHash],
) -> Result<bool, PlanError> {
    let mut footprints = Vec::new();
    footprints
        .try_reserve_exact(changes.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    for ((change, evaluation), victim_hash) in changes.iter().zip(evaluations).zip(victims) {
        let [selected] = evaluation.removals.as_slice() else {
            return Ok(false);
        };
        if selected.cause != RemovalCause::Replacement || selected.hash != *victim_hash {
            return Ok(false);
        }
        let victim_owner = authority
            .entries
            .get(victim_hash)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let OwnedTx::Accepted(victim) = &*victim_owner else {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        };

        let dependency_capacity = change
            .after
            .proof
            .payload()
            .dependencies()
            .len()
            .checked_add(victim.proof.payload().dependencies().len())
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let mut dependencies = HashSet::new();
        dependencies
            .try_reserve(dependency_capacity)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        dependencies.extend(
            change
                .after
                .proof
                .payload()
                .dependencies()
                .keys()
                .iter()
                .cloned(),
        );
        dependencies.extend(victim.proof.payload().dependencies().keys().iter().cloned());

        let mut owners = HashSet::new();
        owners
            .try_reserve(2)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        owners.insert(change.key.clone());
        owners.insert(selected.hash.clone());

        let candidate_owner = OwnedTx::Accepted(change.after.clone());
        let candidate_events = authority
            .collect_dependency_loss_keys(std::iter::once(&candidate_owner))?
            .keys;
        let victim_events = authority
            .collect_dependency_loss_keys(std::iter::once(&*victim_owner))?
            .keys;
        let event_capacity = candidate_events
            .len()
            .checked_add(victim_events.len())
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let mut dependency_events = HashSet::new();
        dependency_events
            .try_reserve(event_capacity)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        dependency_events.extend(candidate_events);
        dependency_events.extend(victim_events);

        let footprint = LogicalLeafFootprint {
            owners,
            dependencies,
            dependency_events,
        };
        if footprints.iter().any(|previous: &LogicalLeafFootprint| {
            !previous.owners.is_disjoint(&footprint.owners)
                || !previous
                    .dependency_events
                    .is_disjoint(&footprint.dependency_events)
                || !previous
                    .dependency_events
                    .is_disjoint(&footprint.dependencies)
                || !footprint
                    .dependency_events
                    .is_disjoint(&previous.dependencies)
        }) {
            return Ok(false);
        }
        footprints.push(footprint);
    }
    Ok(true)
}

fn prepare_removals(
    evaluations: &[MembershipEvaluation],
) -> Result<Vec<Vec<MembershipRemoval>>, PlanError> {
    let mut members = Vec::new();
    members
        .try_reserve_exact(evaluations.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    for evaluation in evaluations {
        let mut removals = Vec::new();
        removals
            .try_reserve_exact(evaluation.removals.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        removals.extend(
            evaluation
                .removals
                .iter()
                .map(|selected| MembershipRemoval::terminal(selected.hash.clone(), selected.cause)),
        );
        members.push(removals);
    }
    Ok(members)
}

fn prepare_ordinary_resources(
    authority: &mut TxPoolAuthority,
    changes: &[IndependentMembershipChange],
) -> Result<Option<ResourceBatchPlan>, PlanError> {
    let mut resource_changes = reserved_vec(changes.len())?;
    resource_changes.extend(changes.iter().map(|change| {
        (
            change.key.clone(),
            Some(change.before.charge_record()),
            Some(change.after.charge_record()),
        )
    }));
    match authority.resources_for_plan().plan_batch(resource_changes) {
        Ok(resource) => Ok(Some(resource)),
        Err(ResourceError::AcceptedLimit) => Ok(None),
        Err(error) => Err(resource_plan_error(error)),
    }
}

fn prepare_ordinary_projection(
    authority: &mut TxPoolAuthority,
    changes: &[IndependentMembershipChange],
) -> Result<ProjectionDelta, PlanError> {
    let (total_inputs, total_dependencies) = changes
        .iter()
        .try_fold((0usize, 0usize), |(inputs, dependencies), change| {
            let footprint = &change.after.proof.payload().footprint;
            Some((
                inputs.checked_add(footprint.inputs().len())?,
                dependencies.checked_add(footprint.dependencies().len())?,
            ))
        })
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
    let proposed_counts = authority.entries.plan_proposed_counts(
        changes
            .iter()
            .map(|change| (&change.key, None, Some(change.after.status()))),
    )?;
    authority.reserve_membership_owner_insertions(
        changes
            .iter()
            .flat_map(|change| change.after.proof.payload().footprint.inputs().iter()),
        changes.iter().map(|change| &change.key),
    )?;

    let mut dependency_insertions = reserved_vec(total_dependencies)?;
    let mut spender_changes = reserved_vec(total_inputs)?;
    let mut causal_insertions = reserved_vec(changes.len())?;
    let mut ancestor_changes = reserved_vec(changes.len())?;
    let mut aggregate_changes = reserved_vec(changes.len())?;
    let mut accepted_order_insertions = reserved_vec(changes.len())?;
    let mut eviction_insertions = reserved_vec(changes.len())?;
    for change in changes {
        dependency_insertions.extend(
            change
                .after
                .proof
                .payload()
                .footprint
                .dependencies()
                .iter()
                .cloned()
                .map(|dependency| DependencyReaderEdge {
                    dependency,
                    reader: change.key.clone(),
                }),
        );
        spender_changes.extend(
            change
                .after
                .proof
                .payload()
                .footprint
                .inputs()
                .iter()
                .cloned()
                .map(|input| (input, Some(change.key.clone()))),
        );
        let ancestor = AncestorAggregate::one(&change.after);
        let aggregate = DescendantAggregate::one(&change.after);
        causal_insertions.push(PreparedCausalNode {
            hash: change.key.clone(),
            parents: HashSet::new(),
            children: HashSet::new(),
        });
        ancestor_changes.push((change.key.clone(), Some(ancestor)));
        aggregate_changes.push((change.key.clone(), Some(aggregate)));
        accepted_order_insertions.push(AcceptedOrderKey::new(&change.after, ancestor));
        eviction_insertions.push(EvictionOrderKey::new(&change.after, aggregate));
    }
    dependency_insertions.sort_unstable();
    dependency_insertions.dedup();
    let (dependency_rows, dependency_row_removals) =
        authority.prepare_dependency_edge_capacity(&[], &dependency_insertions)?;
    spender_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    causal_insertions.sort_unstable_by(|left, right| left.hash.cmp(&right.hash));
    ancestor_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    aggregate_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    accepted_order_insertions.sort_unstable();
    eviction_insertions.sort_unstable();
    Ok(ProjectionDelta {
        spender_changes,
        dependency_changes: dependency_change_log(
            Vec::new(),
            dependency_rows,
            dependency_insertions,
            dependency_row_removals,
        )?,
        causal_changes: causal_change_log(Vec::new(), causal_insertions, Vec::new(), Vec::new())?,
        ancestor_changes,
        aggregate_changes,
        accepted_order_removals: Vec::new(),
        accepted_order_insertions,
        eviction_removals: Vec::new(),
        eviction_insertions,
        proposed_counts,
    })
}

fn prepare_composite_resources(
    authority: &mut TxPoolAuthority,
    changes: &[IndependentMembershipChange],
    member_removals: &mut [Vec<MembershipRemoval>],
) -> Result<Option<ResourceBatchPlan>, PlanError> {
    let current = authority.resources.read(&authority.entries);
    let mut ordered = authority
        .resources
        .ordered_projection(&authority.entries, changes.len())
        .map_err(resource_plan_error)?;
    let removal_count = member_removals
        .iter()
        .try_fold(0usize, |total, removals| total.checked_add(removals.len()));
    let total_changes = removal_count
        .and_then(|removals| removals.checked_add(changes.len()))
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
    let mut resource_changes = Vec::new();
    resource_changes
        .try_reserve_exact(total_changes)
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    let placeholder_sequence = authority.clocks.snapshot().next_sequence;

    for (change, removals) in changes.iter().zip(member_removals.iter_mut()) {
        let retained_history =
            authority.retain_replacement_history(&change.after, removals, placeholder_sequence)?;
        if !retained_history {
            removals.iter_mut().for_each(MembershipRemoval::terminalize);
        }
        let candidate = (
            Some(change.before.charge_record()),
            Some(change.after.charge_record()),
        );
        let first = match removals.as_slice() {
            [] => ordered.replace(current, candidate.0, candidate.1),
            [removal] => {
                let victim = authority
                    .entries
                    .get(&removal.hash)
                    .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
                ordered.replace_set(
                    current,
                    &[
                        candidate,
                        (
                            Some(victim.charge_record()),
                            removal.after().map(OwnedTx::charge_record),
                        ),
                    ],
                )
            }
            _ => return Err(PlanError::Fault(AuthorityFault::MembershipProjection)),
        };
        let result = match first {
            Err(ResourceError::PreAcceptedLimit | ResourceError::ReplacementHistoryLimit)
                if retained_history =>
            {
                removals.iter_mut().for_each(MembershipRemoval::terminalize);
                let [removal] = removals.as_slice() else {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                };
                let victim = authority
                    .entries
                    .get(&removal.hash)
                    .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
                ordered.replace_set(
                    current,
                    &[
                        (
                            Some(change.before.charge_record()),
                            Some(change.after.charge_record()),
                        ),
                        (Some(victim.charge_record()), None),
                    ],
                )
            }
            other => other,
        };
        match result {
            Ok(()) => {}
            Err(ResourceError::AcceptedLimit) => return Ok(None),
            Err(error) => return Err(resource_plan_error(error)),
        }

        resource_changes.push((
            change.key.clone(),
            Some(change.before.charge_record()),
            Some(change.after.charge_record()),
        ));
        for removal in removals {
            let victim = authority
                .entries
                .get(&removal.hash)
                .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
            resource_changes.push((
                removal.hash.clone(),
                Some(victim.charge_record()),
                removal.after().map(OwnedTx::charge_record),
            ));
        }
    }

    authority
        .resources_for_plan()
        .plan_batch(resource_changes)
        .map(Some)
        .map_err(resource_plan_error)
}

fn resource_plan_error(error: ResourceError) -> PlanError {
    match error {
        ResourceError::Allocation => PlanError::Backpressure(Backpressure::Allocation),
        ResourceError::Arithmetic
        | ResourceError::PreAcceptedLimit
        | ResourceError::RemoteLimit
        | ResourceError::PeerLimit(_)
        | ResourceError::ReplacementHistoryLimit
        | ResourceError::AcceptedLimit
        | ResourceError::ExistingChargeMismatch
        | ResourceError::DuplicateChange
        | ResourceError::ComputeEnvelope
        | ResourceError::AttributionMismatch
        | ResourceError::CapacityBankFault => PlanError::Fault(AuthorityFault::ResourceProjection),
    }
}

fn prepare_composite_projection(
    authority: &mut TxPoolAuthority,
    changes: &[IndependentMembershipChange],
    evaluations: Vec<MembershipEvaluation>,
) -> Result<ProjectionDelta, PlanError> {
    let mut total_removals = 0usize;
    let mut total_inputs = 0usize;
    let mut total_dependencies = 0usize;
    for (change, evaluation) in changes.iter().zip(&evaluations) {
        checked_accumulate(&mut total_removals, evaluation.removals.len())?;
        let footprint = &change.after.proof.payload().footprint;
        checked_accumulate(&mut total_inputs, footprint.inputs().len())?;
        checked_accumulate(&mut total_dependencies, footprint.dependencies().len())?;
        for removal in &evaluation.removals {
            let entry = authority.accepted_entry(&removal.hash)?;
            checked_accumulate(
                &mut total_inputs,
                entry.proof.payload().footprint.inputs().len(),
            )?;
            checked_accumulate(
                &mut total_dependencies,
                entry.proof.payload().footprint.dependencies().len(),
            )?;
        }
    }

    let status_capacity = changes
        .len()
        .checked_add(total_removals)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
    let mut status_changes = reserved_vec(status_capacity)?;
    let mut spender_changes = reserved_vec(total_inputs)?;
    let mut dependency_reader_removals = reserved_vec(total_dependencies)?;
    let mut dependency_reader_insertions = reserved_vec(total_dependencies)?;
    let mut causal_node_insertions = reserved_vec::<PreparedCausalNode>(changes.len())?;
    let mut causal_node_removals = reserved_vec(total_removals)?;
    // The admitted shapes have exactly one candidate aggregate and at most
    // one leaf-victim removal per member. Canonical evaluation owns the
    // values; this builder reserves only their complete mechanical journals.
    let mut aggregate_changes = reserved_vec(status_capacity)?;
    let mut ancestor_changes = reserved_vec(status_capacity)?;
    let mut accepted_order_removals = reserved_vec::<AcceptedOrderKey>(total_removals)?;
    let mut accepted_order_insertions = reserved_vec::<AcceptedOrderKey>(changes.len())?;
    let mut eviction_removals = reserved_vec::<EvictionOrderKey>(total_removals)?;
    let mut eviction_insertions = reserved_vec::<EvictionOrderKey>(changes.len())?;

    for (change, evaluation) in changes.iter().zip(evaluations) {
        let MembershipEvaluation {
            removals,
            candidate_parents,
            candidate_children,
            aggregate,
        } = evaluation;
        if !candidate_parents.is_empty() || !candidate_children.is_empty() {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        for removal in &removals {
            let entry = authority.accepted_entry(&removal.hash)?;
            let parents = authority
                .membership
                .parents(&removal.hash)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let children = authority
                .membership
                .children(&removal.hash)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            if !parents.is_empty() || !children.is_empty() {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            status_changes.push((removal.hash.clone(), Some(entry.status()), None));
            for input in entry.proof.payload().footprint.inputs() {
                if authority.membership.spender(input) != Some(removal.hash.clone()) {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                spender_changes.push((input.clone(), None));
            }
            for dependency in entry.proof.payload().footprint.dependencies() {
                if !authority
                    .membership
                    .dependency_reader_row_facts(dependency, &removal.hash)
                    .is_some_and(|(_, present)| present)
                {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                dependency_reader_removals.push(DependencyReaderEdge {
                    dependency: dependency.clone(),
                    reader: removal.hash.clone(),
                });
            }
            causal_node_removals.push(removal.hash.clone());
        }
        status_changes.push((change.key.clone(), None, Some(change.after.status())));
        for input in change.after.proof.payload().footprint.inputs() {
            spender_changes.push((input.clone(), Some(change.key.clone())));
        }
        for dependency in change.after.proof.payload().footprint.dependencies() {
            dependency_reader_insertions.push(DependencyReaderEdge {
                dependency: dependency.clone(),
                reader: change.key.clone(),
            });
        }
        causal_node_insertions.extend(authority.prepare_causal_edge_capacity(
            &change.key,
            &candidate_parents,
            &candidate_children,
            &[],
        )?);
        aggregate_changes.extend(aggregate.changes);
        ancestor_changes.extend(aggregate.ancestor_changes);
        accepted_order_removals.extend(aggregate.accepted_order_removals);
        accepted_order_insertions.extend(aggregate.accepted_order_insertions);
        eviction_removals.extend(aggregate.eviction_removals);
        eviction_insertions.extend(aggregate.eviction_insertions);
    }

    let proposed_counts = authority.entries.plan_proposed_counts(
        status_changes
            .iter()
            .map(|(hash, before, after)| (hash, *before, *after)),
    )?;
    authority.reserve_membership_owner_insertions(
        changes
            .iter()
            .flat_map(|change| change.after.proof.payload().footprint.inputs().iter()),
        changes.iter().map(|change| &change.key),
    )?;
    dependency_reader_removals.sort_unstable();
    dependency_reader_removals.dedup();
    dependency_reader_insertions.sort_unstable();
    dependency_reader_insertions.dedup();
    let (dependency_row_insertions, dependency_row_removals) = authority
        .prepare_dependency_edge_capacity(
            &dependency_reader_removals,
            &dependency_reader_insertions,
        )?;
    let dependency_changes = dependency_change_log(
        dependency_reader_removals,
        dependency_row_insertions,
        dependency_reader_insertions,
        dependency_row_removals,
    )?;

    // A candidate owns the input released by its victim. Canonicalize the
    // pair to one final spender without a post-hoc delta merge.
    spender_changes.sort_unstable_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.is_none().cmp(&right.1.is_none()))
    });
    spender_changes.dedup_by(|later, earlier| later.0 == earlier.0);
    causal_node_insertions.sort_unstable_by(|left, right| left.hash.cmp(&right.hash));
    causal_node_removals.sort_unstable();
    if causal_node_removals
        .array_windows::<2>()
        .any(|[left, right]| left == right)
    {
        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
    }
    let causal_changes = causal_change_log(
        Vec::new(),
        causal_node_insertions,
        Vec::new(),
        causal_node_removals,
    )?;

    aggregate_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    ancestor_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    if aggregate_changes
        .array_windows::<2>()
        .any(|[left, right]| left.0 == right.0)
        || ancestor_changes
            .array_windows::<2>()
            .any(|[left, right]| left.0 == right.0)
    {
        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
    }
    accepted_order_removals.sort_unstable();
    accepted_order_insertions.sort_unstable();
    eviction_removals.sort_unstable();
    eviction_insertions.sort_unstable();

    Ok(ProjectionDelta {
        spender_changes,
        dependency_changes,
        causal_changes,
        ancestor_changes,
        aggregate_changes,
        accepted_order_removals,
        accepted_order_insertions,
        eviction_removals,
        eviction_insertions,
        proposed_counts,
    })
}
