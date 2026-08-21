use super::{ComponentLimitKind, MembershipReject, ReplacementPolicy};
use crate::authority::{
    plan::{AuthorityFault, PlanError, TxPoolAuthority},
    state::{AcceptedEntry, OwnedTx, RawTxHash},
};
use ckb_types::core::Capacity;
use std::collections::HashSet;

pub(super) fn replacement_removals(
    authority: &TxPoolAuthority,
    candidate: &AcceptedEntry,
) -> Result<Vec<RawTxHash>, PlanError> {
    let footprint = &candidate.proof.payload().footprint;
    let mut direct = Vec::new();
    direct
        .try_reserve_exact(footprint.inputs().len())
        .map_err(|_| PlanError::Backpressure(super::super::Backpressure::Allocation))?;
    let mut first_conflict = None;
    for input in footprint.inputs() {
        if let Some(spender) = authority.membership.spender(input) {
            direct.push(spender.clone());
            first_conflict.get_or_insert_with(|| input.clone());
        }
    }
    if direct.is_empty() {
        return Ok(Vec::new());
    }
    direct.sort_unstable();
    direct.dedup();

    let minimum_rate = match authority.membership_config.replacement {
        ReplacementPolicy::Disabled => {
            return Err(PlanError::Membership(MembershipReject::InputConflict(
                first_conflict.ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?,
            )));
        }
        ReplacementPolicy::Enabled { minimum_rate } => minimum_rate,
    };

    validate_no_new_unconfirmed_inputs(authority, candidate, &direct)?;
    let removals = authority.bounded_descendant_postorder(
        &direct,
        &HashSet::new(),
        authority.membership_config.max_component,
        ComponentLimitKind::Replacement,
    )?;
    let mut removal_set = HashSet::new();
    removal_set
        .try_reserve(removals.len())
        .map_err(|_| PlanError::Backpressure(super::super::Backpressure::Allocation))?;
    removal_set.extend(removals.iter().cloned());
    validate_descendant_overlap(authority, candidate, &direct, &removal_set)?;
    validate_no_victim_dependencies(candidate, &removal_set)?;
    validate_replacement_fee(authority, candidate, &removals, minimum_rate)?;
    Ok(removals)
}

fn validate_no_new_unconfirmed_inputs(
    authority: &TxPoolAuthority,
    candidate: &AcceptedEntry,
    direct: &[RawTxHash],
) -> Result<(), PlanError> {
    let mut replaced_inputs = HashSet::new();
    let input_capacity = direct.iter().try_fold(0usize, |total, hash| {
        total
            .checked_add(
                accepted_entry(authority, hash)?
                    .proof
                    .payload()
                    .footprint
                    .inputs()
                    .len(),
            )
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
    })?;
    replaced_inputs
        .try_reserve(input_capacity)
        .map_err(|_| PlanError::Backpressure(super::super::Backpressure::Allocation))?;
    for hash in direct {
        let entry = accepted_entry(authority, hash)?;
        replaced_inputs.extend(entry.proof.payload().footprint.inputs().iter().cloned());
    }
    for input in candidate.proof.payload().footprint.inputs() {
        if !replaced_inputs.contains(input) && !candidate.proof.is_chain_input(input) {
            return Err(PlanError::Membership(
                MembershipReject::NewUnconfirmedInput(input.clone()),
            ));
        }
    }
    Ok(())
}

fn validate_descendant_overlap(
    authority: &TxPoolAuthority,
    candidate: &AcceptedEntry,
    direct: &[RawTxHash],
    removal_set: &HashSet<RawTxHash>,
) -> Result<(), PlanError> {
    let mut descendants = HashSet::new();
    descendants
        .try_reserve(removal_set.len())
        .map_err(|_| PlanError::Backpressure(super::super::Backpressure::Allocation))?;
    descendants.extend(
        removal_set
            .iter()
            .filter(|hash| direct.binary_search(hash).is_err())
            .cloned(),
    );
    for input in candidate.proof.payload().footprint.inputs() {
        if descendants.contains(&RawTxHash(input.tx_hash())) {
            return Err(PlanError::Membership(
                MembershipReject::InputFromDescendant(input.clone()),
            ));
        }
    }

    let mut parents = HashSet::new();
    parents
        .try_reserve(candidate.proof.payload().footprint.inputs().len())
        .map_err(|_| PlanError::Backpressure(super::super::Backpressure::Allocation))?;
    for input in candidate.proof.payload().footprint.inputs() {
        let parent = RawTxHash(input.tx_hash());
        if authority
            .entries
            .get(&parent)
            .is_some_and(|entry| matches!(entry, OwnedTx::Accepted(_)))
        {
            parents.insert(parent);
        }
    }
    let ancestors = collect_overlap_ancestors(authority, &parents)?;
    if !ancestors.is_disjoint(&descendants) {
        return Err(PlanError::Membership(
            MembershipReject::AncestorDescendantOverlap,
        ));
    }
    Ok(())
}

fn collect_overlap_ancestors(
    authority: &TxPoolAuthority,
    parents: &HashSet<RawTxHash>,
) -> Result<HashSet<RawTxHash>, PlanError> {
    // Each accepted parent was admitted under the per-transaction ancestor
    // bound. The union may be larger than that bound when an RBF candidate
    // conflicts with several independent roots, but it cannot exceed the sum
    // of those bounded closures.
    let limit = parents
        .len()
        .checked_mul(authority.membership_config.max_ancestors)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
    let mut ancestors = HashSet::new();
    ancestors
        .try_reserve(limit)
        .map_err(|_| PlanError::Backpressure(super::super::Backpressure::Allocation))?;
    let mut frontier = Vec::new();
    frontier
        .try_reserve(limit)
        .map_err(|_| PlanError::Backpressure(super::super::Backpressure::Allocation))?;
    for parent in parents {
        if ancestors.insert(parent.clone()) {
            frontier.push(parent.clone());
        }
    }
    while let Some(hash) = frontier.pop() {
        let grandparents = authority
            .membership
            .parents(&hash)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        for grandparent in grandparents {
            if ancestors.insert(grandparent.clone()) {
                if ancestors.len() > limit {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                frontier.push(grandparent.clone());
            }
        }
    }
    Ok(ancestors)
}

fn validate_no_victim_dependencies(
    candidate: &AcceptedEntry,
    removal_set: &HashSet<RawTxHash>,
) -> Result<(), PlanError> {
    for dependency in candidate.proof.payload().footprint.dependencies() {
        if removal_set.contains(&RawTxHash(dependency.tx_hash())) {
            return Err(PlanError::Membership(MembershipReject::DependencyOnVictim(
                dependency.clone(),
            )));
        }
    }
    Ok(())
}

fn validate_replacement_fee(
    authority: &TxPoolAuthority,
    candidate: &AcceptedEntry,
    removals: &[RawTxHash],
    minimum_rate: ckb_types::core::FeeRate,
) -> Result<(), PlanError> {
    let replaced_fee = removals.iter().try_fold(Capacity::zero(), |sum, hash| {
        let fee = accepted_entry(authority, hash)?.proof.metrics().fee;
        sum.safe_add(fee)
            .map_err(|_| PlanError::Membership(MembershipReject::ReplacementFeeOverflow))
    })?;
    let serialized_bytes = u64::try_from(candidate.proof.metrics().cost.serialized_bytes)
        .map_err(|_| PlanError::Membership(MembershipReject::ReplacementFeeOverflow))?;
    let required = replaced_fee
        .safe_add(minimum_rate.fee(serialized_bytes))
        .map_err(|_| PlanError::Membership(MembershipReject::ReplacementFeeOverflow))?;
    let actual = candidate.proof.metrics().fee;
    if actual < required {
        return Err(PlanError::Membership(
            MembershipReject::InsufficientReplacementFee { actual, required },
        ));
    }
    Ok(())
}

fn accepted_entry<'a>(
    authority: &'a TxPoolAuthority,
    hash: &RawTxHash,
) -> Result<&'a AcceptedEntry, PlanError> {
    match authority.entries.get(hash) {
        Some(OwnedTx::Accepted(entry)) => Ok(entry),
        Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
            Err(PlanError::Fault(AuthorityFault::MembershipProjection))
        }
    }
}
