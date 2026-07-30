use super::{
    DependencyReaderEdge, DescendantAggregate, EvictionOrderKey, PreparedCausalNode,
    ProjectionDelta, causal_change_log, dependency_change_log,
};
use crate::authority::{
    plan::{AuthorityFault, Backpressure, PlanError, TxPoolAuthority},
    resources::{ResourceBatchPlan, ResourceError},
    state::{AcceptedEntry, OwnedTx, PreAcceptedEntry, RawTxHash},
};
use ckb_types::packed::OutPoint;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub(in crate::authority::plan) struct IndependentMembershipChange {
    pub(in crate::authority::plan) key: RawTxHash,
    pub(in crate::authority::plan) before: PreAcceptedEntry,
    pub(in crate::authority::plan) after: AcceptedEntry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) enum IndependentCoupling {
    InputNotChainBacked(OutPoint),
    AcceptedSpender(OutPoint),
    AcceptedConditionalEdge(OutPoint),
    PoolParent(RawTxHash),
    CohortInputConflict(OutPoint),
    CohortConditionalEdge(OutPoint),
    CohortCausalEdge(RawTxHash),
    AcceptedChild(RawTxHash),
    AcceptedCapacity,
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
    Coupled(IndependentCoupling),
}

pub(in crate::authority::plan) fn prepare_independent_membership(
    authority: &mut TxPoolAuthority,
    changes: &[IndependentMembershipChange],
) -> Result<IndependentMembershipOutcome, PlanError> {
    if let Some(coupling) = classify(authority, changes)? {
        return Ok(IndependentMembershipOutcome::Coupled(coupling));
    }

    let resource = match prepare_resources(authority, changes)? {
        Some(resource) => resource,
        None => {
            return Ok(IndependentMembershipOutcome::Coupled(
                IndependentCoupling::AcceptedCapacity,
            ));
        }
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
) -> Result<Option<IndependentCoupling>, PlanError> {
    for (index, change) in changes.iter().enumerate() {
        validate_owner(authority, change)?;
        let footprint = &change.after.verified.payload().footprint;

        for input in footprint.inputs() {
            if authority.membership.spender(input).is_some() {
                return Ok(Some(IndependentCoupling::AcceptedSpender(input.clone())));
            }
            if authority
                .membership
                .dependency_readers(input)
                .is_some_and(|readers| !readers.is_empty())
            {
                return Ok(Some(IndependentCoupling::AcceptedConditionalEdge(
                    input.clone(),
                )));
            }
            let producer = RawTxHash(input.tx_hash());
            if is_accepted(authority, &producer) {
                return Ok(Some(IndependentCoupling::PoolParent(producer)));
            }
            if changes
                .iter()
                .enumerate()
                .any(|(other, candidate)| other != index && candidate.key == producer)
            {
                return Ok(Some(IndependentCoupling::CohortCausalEdge(producer)));
            }
            if !change.after.verified.payload().is_chain_input(input) {
                return Ok(Some(IndependentCoupling::InputNotChainBacked(
                    input.clone(),
                )));
            }
        }

        for dependency in footprint.dependencies() {
            if authority.membership.spender(dependency).is_some() {
                return Ok(Some(IndependentCoupling::AcceptedConditionalEdge(
                    dependency.clone(),
                )));
            }
            let producer = RawTxHash(dependency.tx_hash());
            if is_accepted(authority, &producer) {
                return Ok(Some(IndependentCoupling::PoolParent(producer)));
            }
            if changes
                .iter()
                .enumerate()
                .any(|(other, candidate)| other != index && candidate.key == producer)
            {
                return Ok(Some(IndependentCoupling::CohortCausalEdge(producer)));
            }
        }

        if let Some(child) = accepted_child(authority, change) {
            return Ok(Some(IndependentCoupling::AcceptedChild(child)));
        }

        for other in changes.iter().skip(index + 1) {
            if change.after.record.identity.proposal == other.after.record.identity.proposal {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            if let Some(input) = first_shared_input(change, other) {
                return Ok(Some(IndependentCoupling::CohortInputConflict(input)));
            }
            if let Some(out_point) = first_conditional_edge(change, other) {
                return Ok(Some(IndependentCoupling::CohortConditionalEdge(out_point)));
            }
        }
    }
    Ok(None)
}

fn validate_owner(
    authority: &TxPoolAuthority,
    change: &IndependentMembershipChange,
) -> Result<(), PlanError> {
    let before = &change.before;
    let after = &change.after;
    if before.record.identity.raw != change.key
        || after.record.identity != before.record.identity
        || after.record.ingress != before.record.ingress
        || after.record.blame != before.record.blame
        || after.record.class != before.record.class
        || after.record.arrival != before.record.arrival
        || after.verified.chain_epoch() != authority.chain_epoch
    {
        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
    }
    match authority.entries.get(&change.key) {
        Some(OwnedTx::PreAccepted(current))
            if current.record.version == before.record.version
                && current.record.identity == before.record.identity => {}
        Some(OwnedTx::PreAccepted(_)) | Some(OwnedTx::Accepted(_)) | None => {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
    }
    if authority.by_proposal.get(&before.record.identity.proposal) != Some(&change.key) {
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

fn accepted_child(
    authority: &TxPoolAuthority,
    change: &IndependentMembershipChange,
) -> Option<RawTxHash> {
    authority
        .accepted_children_of_candidate(&change.after)
        .min()
        .cloned()
}

fn first_shared_input(
    left: &IndependentMembershipChange,
    right: &IndependentMembershipChange,
) -> Option<OutPoint> {
    let right_inputs = right.after.verified.payload().footprint.inputs();
    left.after
        .verified
        .payload()
        .footprint
        .inputs()
        .iter()
        .find(|input| right_inputs.binary_search(input).is_ok())
        .cloned()
}

fn first_conditional_edge(
    left: &IndependentMembershipChange,
    right: &IndependentMembershipChange,
) -> Option<OutPoint> {
    let left_footprint = &left.after.verified.payload().footprint;
    let right_footprint = &right.after.verified.payload().footprint;
    left_footprint
        .inputs()
        .iter()
        .find(|input| right_footprint.dependencies().binary_search(input).is_ok())
        .or_else(|| {
            right_footprint
                .inputs()
                .iter()
                .find(|input| left_footprint.dependencies().binary_search(input).is_ok())
        })
        .cloned()
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
    match authority.resources.plan_batch(resource_changes) {
        Ok(resource) => Ok(Some(resource)),
        Err(ResourceError::AcceptedLimit) => Ok(None),
        Err(ResourceError::Allocation) => Err(PlanError::Backpressure(Backpressure::Allocation)),
        Err(
            ResourceError::Arithmetic
            | ResourceError::PreAcceptedLimit
            | ResourceError::RemoteLimit
            | ResourceError::PeerLimit(_)
            | ResourceError::ExistingChargeMismatch
            | ResourceError::DuplicateChange,
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
            let footprint = &change.after.verified.payload().footprint;
            Some((
                inputs.checked_add(footprint.inputs().len())?,
                dependencies.checked_add(footprint.dependencies().len())?,
            ))
        })
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;

    let mut counts = authority.membership.counts;
    for change in changes {
        counts = counts
            .checked_add(change.after.status)
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
    }

    authority
        .membership
        .spenders
        .try_reserve(total_inputs)
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    authority
        .membership
        .parents
        .try_reserve(changes.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    authority
        .membership
        .children
        .try_reserve(changes.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    authority
        .membership
        .descendant_aggregates
        .try_reserve(changes.len())
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;

    let mut dependency_reader_insertions = Vec::new();
    dependency_reader_insertions
        .try_reserve(total_dependencies)
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    dependency_reader_insertions.extend(changes.iter().flat_map(|change| {
        change
            .after
            .verified
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
                .verified
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
    for change in changes {
        let aggregate = DescendantAggregate::one(&change.after);
        causal_node_insertions.push(PreparedCausalNode {
            hash: change.key.clone(),
            parents: HashSet::new(),
            children: HashSet::new(),
        });
        aggregate_changes.push((change.key.clone(), Some(aggregate)));
        eviction_insertions.push(EvictionOrderKey::new(&change.after, aggregate));
    }
    causal_node_insertions.sort_unstable_by(|left, right| left.hash.cmp(&right.hash));
    aggregate_changes.sort_unstable_by(|left, right| left.0.cmp(&right.0));
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
        aggregate_changes,
        eviction_removals: Vec::new(),
        eviction_insertions,
        counts,
    })
}
