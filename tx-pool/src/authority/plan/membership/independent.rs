use super::{
    AcceptedOrderKey, AncestorAggregate, DependencyReaderEdge, DescendantAggregate,
    EvictionOrderKey, PreparedCausalNode, ProjectionDelta, causal_change_log,
    dependency_change_log,
};
use crate::authority::{
    plan::{AuthorityFault, Backpressure, PlanError, TxPoolAuthority},
    resources::{ResourceBatchPlan, ResourceError},
    state::{AcceptedEntry, OwnedTx, PreAcceptedEntry, RawTxHash},
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
}

#[expect(
    clippy::large_enum_variant,
    reason = "the prepared variant owns already-fallible Plan storage; boxing would add an independent hot-path allocation"
)]
pub(in crate::authority::plan) enum IndependentMembershipOutcome {
    Prepared(PreparedIndependentMembership),
    Coupled,
}

pub(in crate::authority::plan) fn prepare_independent_membership(
    authority: &mut TxPoolAuthority,
    changes: &[IndependentMembershipChange],
) -> Result<IndependentMembershipOutcome, PlanError> {
    if classify(authority, changes)? {
        return Ok(IndependentMembershipOutcome::Coupled);
    }

    let resource = match prepare_resources(authority, changes)? {
        Some(resource) => resource,
        None => return Ok(IndependentMembershipOutcome::Coupled),
    };
    let projection = prepare_projection(authority, changes)?;
    Ok(IndependentMembershipOutcome::Prepared(
        PreparedIndependentMembership {
            resource,
            projection,
        },
    ))
}

fn classify(
    authority: &TxPoolAuthority,
    changes: &[IndependentMembershipChange],
) -> Result<bool, PlanError> {
    for (index, change) in changes.iter().enumerate() {
        validate_owner(authority, change)?;
        let footprint = &change.after.proof.payload().footprint;

        for input in footprint.inputs() {
            if authority.membership.spender(input).is_some() {
                return Ok(true);
            }
            if authority
                .membership
                .dependency_readers(input)
                .is_some_and(|readers| !readers.is_empty())
            {
                return Ok(true);
            }
            let producer = RawTxHash(input.tx_hash());
            if is_accepted(authority, &producer) {
                return Ok(true);
            }
            if changes
                .iter()
                .enumerate()
                .any(|(other, candidate)| other != index && candidate.key == producer)
            {
                return Ok(true);
            }
            if !change.after.proof.is_chain_input(input) {
                return Ok(true);
            }
        }

        for dependency in footprint.dependencies() {
            if authority.membership.spender(dependency).is_some() {
                return Ok(true);
            }
            let producer = RawTxHash(dependency.tx_hash());
            if is_accepted(authority, &producer) {
                return Ok(true);
            }
            if changes
                .iter()
                .enumerate()
                .any(|(other, candidate)| other != index && candidate.key == producer)
            {
                return Ok(true);
            }
            if !change.after.proof.is_chain_dependency(dependency) {
                return Ok(true);
            }
        }

        if has_accepted_child(authority, change) {
            return Ok(true);
        }

        // `index` comes from this exact slice. Skipping the indexed element
        // separately avoids arithmetic or saturation in the pairwise proof.
        for other in changes.iter().skip(index).skip(1) {
            if change.after.record.identity.proposal == other.after.record.identity.proposal {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            if has_shared_input(change, other) {
                return Ok(true);
            }
            if has_conditional_edge(change, other) {
                return Ok(true);
            }
        }
    }
    Ok(false)
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
    match authority.entries.get(&change.key) {
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

fn prepare_resources(
    authority: &mut TxPoolAuthority,
    changes: &[IndependentMembershipChange],
) -> Result<Option<ResourceBatchPlan>, PlanError> {
    let mut resource_changes = Vec::new();
    resource_changes
        .try_reserve(changes.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
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
        Err(ResourceError::Allocation) => Err(PlanError::Backpressure(Backpressure::Allocation)),
        Err(
            ResourceError::Arithmetic
            | ResourceError::PreAcceptedLimit
            | ResourceError::RemoteLimit
            | ResourceError::PeerLimit(_)
            | ResourceError::ReplacementHistoryLimit
            | ResourceError::ExistingChargeMismatch
            | ResourceError::DuplicateChange
            | ResourceError::ComputeEnvelope
            | ResourceError::AttributionMismatch,
        ) => Err(PlanError::Fault(AuthorityFault::ResourceProjection)),
    }
}

fn prepare_projection(
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

    let status_counts = authority.entries.plan_status_counts(
        changes
            .iter()
            .map(|change| (&change.key, None, Some(change.after.status()))),
    )?;

    authority.reserve_membership_owner_insertions(total_inputs, changes.len())?;

    let mut dependency_reader_insertions = Vec::new();
    dependency_reader_insertions
        .try_reserve(total_dependencies)
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    dependency_reader_insertions.extend(changes.iter().flat_map(|change| {
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
            })
    }));
    dependency_reader_insertions.sort_unstable();
    dependency_reader_insertions.dedup();
    let (dependency_row_insertions, dependency_row_removals) =
        authority.prepare_dependency_edge_capacity(&[], &dependency_reader_insertions)?;

    let mut spender_changes = Vec::new();
    spender_changes
        .try_reserve(total_inputs)
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    for change in changes {
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
    }
    spender_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    let mut causal_node_insertions = Vec::new();
    causal_node_insertions
        .try_reserve(changes.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    let mut aggregate_changes = Vec::new();
    aggregate_changes
        .try_reserve(changes.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    let mut eviction_insertions = Vec::new();
    eviction_insertions
        .try_reserve(changes.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    let mut ancestor_changes = Vec::new();
    ancestor_changes
        .try_reserve(changes.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    let mut accepted_order_insertions = Vec::new();
    accepted_order_insertions
        .try_reserve(changes.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    for change in changes {
        let aggregate = DescendantAggregate::one(&change.after);
        let ancestor = AncestorAggregate::one(&change.after);
        causal_node_insertions.push(PreparedCausalNode {
            hash: change.key.clone(),
            parents: HashSet::new(),
            children: HashSet::new(),
        });
        ancestor_changes.push((change.key.clone(), Some(ancestor)));
        aggregate_changes.push((change.key.clone(), Some(aggregate)));
        accepted_order_insertions.push(AcceptedOrderKey::new(&change.after, ancestor));
        eviction_insertions.push(EvictionOrderKey::new(&change.after, aggregate));
    }
    causal_node_insertions.sort_unstable_by(|left, right| left.hash.cmp(&right.hash));
    ancestor_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    aggregate_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    accepted_order_insertions.sort_unstable();
    eviction_insertions.sort_unstable();
    let dependency_changes = dependency_change_log(
        Vec::new(),
        dependency_row_insertions,
        dependency_reader_insertions,
        dependency_row_removals,
    )?;
    let causal_changes =
        causal_change_log(Vec::new(), causal_node_insertions, Vec::new(), Vec::new())?;

    Ok(ProjectionDelta {
        spender_changes,
        dependency_changes,
        causal_changes,
        ancestor_changes,
        aggregate_changes,
        accepted_order_removals: Vec::new(),
        accepted_order_insertions,
        eviction_removals: Vec::new(),
        eviction_insertions,
        status_counts,
    })
}
