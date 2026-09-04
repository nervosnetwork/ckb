use super::{ComponentLimitKind, MembershipReject, PolicyContext, PolicyMode, ReplacementPolicy};
use crate::authority::{
    plan::{AuthorityFault, PlanError},
    shard::OwnerEntryKind,
    state::{AcceptedEntry, RawTxHash},
};
use ckb_types::core::Capacity;
use std::collections::HashSet;

pub(super) fn replacement_removals<Mode>(
    candidate: &AcceptedEntry,
    reader: &mut PolicyContext<'_, Mode>,
) -> Result<Vec<RawTxHash>, PlanError>
where
    Mode: PolicyMode,
{
    let config = reader.config();
    let footprint = &candidate.proof.payload().footprint;
    let mut direct = Vec::new();
    let mut first_conflict = None;
    for input in footprint.inputs() {
        if let Some(spender) = reader.observe_spender(input)? {
            if direct.is_empty() {
                direct.reserve_exact(footprint.inputs().len());
            }
            direct.push(spender.clone());
            first_conflict.get_or_insert_with(|| input.clone());
        }
    }
    if direct.is_empty() {
        return Ok(Vec::new());
    }
    direct.sort_unstable();
    direct.dedup();

    let minimum_rate = match config.replacement {
        ReplacementPolicy::Disabled => {
            return Err(PlanError::Membership(MembershipReject::InputConflict(
                first_conflict.ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?,
            )));
        }
        ReplacementPolicy::Enabled { minimum_rate } => minimum_rate,
    };

    validate_no_new_unconfirmed_inputs(candidate, &direct, reader)?;
    let removals = if let [root] = direct.as_slice() {
        let children = reader
            .observe_children(root)?
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        if children.is_empty() {
            // The general postorder also validates both relation rows. Keep
            // that fault boundary while avoiding its frontier, closure map,
            // heap and output sort for the dominant leaf-victim case.
            reader
                .observe_parents(root)?
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            vec![root.clone()]
        } else {
            super::TxPoolAuthority::bounded_descendant_postorder_with_reader(
                &direct,
                &HashSet::new(),
                config.max_component,
                ComponentLimitKind::Replacement,
                reader,
            )?
            .ordered
        }
    } else {
        super::TxPoolAuthority::bounded_descendant_postorder_with_reader(
            &direct,
            &HashSet::new(),
            config.max_component,
            ComponentLimitKind::Replacement,
            reader,
        )?
        .ordered
    };
    let mut removal_set = HashSet::with_capacity(removals.len());
    removal_set.extend(removals.iter().cloned());
    validate_descendant_overlap(candidate, &direct, &removal_set, reader)?;
    validate_no_victim_dependencies(candidate, &removal_set)?;
    validate_replacement_fee(candidate, &removals, minimum_rate, reader)?;
    Ok(removals)
}

fn validate_no_new_unconfirmed_inputs<Mode>(
    candidate: &AcceptedEntry,
    direct: &[RawTxHash],
    reader: &mut PolicyContext<'_, Mode>,
) -> Result<(), PlanError>
where
    Mode: PolicyMode,
{
    let mut replaced_inputs = HashSet::new();
    let input_capacity = direct.iter().try_fold(0usize, |total, hash| {
        total
            .checked_add(
                accepted_entry(hash, reader)?
                    .proof
                    .payload()
                    .footprint
                    .inputs()
                    .len(),
            )
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
    })?;
    replaced_inputs.reserve(input_capacity);
    for hash in direct {
        let entry = accepted_entry(hash, reader)?;
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

fn validate_descendant_overlap<Mode>(
    candidate: &AcceptedEntry,
    direct: &[RawTxHash],
    removal_set: &HashSet<RawTxHash>,
    reader: &mut PolicyContext<'_, Mode>,
) -> Result<(), PlanError>
where
    Mode: PolicyMode,
{
    if removal_set.len() == direct.len() && direct.iter().all(|hash| removal_set.contains(hash)) {
        return Ok(());
    }
    let mut descendants = HashSet::with_capacity(removal_set.len());
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

    let mut parents = HashSet::with_capacity(candidate.proof.payload().footprint.inputs().len());
    for input in candidate.proof.payload().footprint.inputs() {
        let parent = RawTxHash(input.tx_hash());
        if reader
            .observe_owner(&parent)?
            .is_some_and(|fact| fact.kind == OwnerEntryKind::Accepted)
        {
            parents.insert(parent);
        }
    }
    let ancestors = collect_overlap_ancestors(&parents, reader)?;
    if !ancestors.is_disjoint(&descendants) {
        return Err(PlanError::Membership(
            MembershipReject::AncestorDescendantOverlap,
        ));
    }
    Ok(())
}

fn collect_overlap_ancestors<Mode>(
    parents: &HashSet<RawTxHash>,
    reader: &mut PolicyContext<'_, Mode>,
) -> Result<HashSet<RawTxHash>, PlanError>
where
    Mode: PolicyMode,
{
    // Each accepted parent was admitted under the per-transaction ancestor
    // bound. The union may be larger than that bound when an RBF candidate
    // conflicts with several independent roots, but it cannot exceed the sum
    // of those bounded closures.
    let limit = parents
        .len()
        .checked_mul(reader.config().max_ancestors)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
    let mut ancestors = HashSet::with_capacity(limit);
    let mut frontier = Vec::with_capacity(limit);
    for parent in parents {
        if ancestors.insert(parent.clone()) {
            frontier.push(parent.clone());
        }
    }
    while let Some(hash) = frontier.pop() {
        let grandparents = reader
            .observe_parents(&hash)?
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

fn validate_replacement_fee<Mode>(
    candidate: &AcceptedEntry,
    removals: &[RawTxHash],
    minimum_rate: ckb_types::core::FeeRate,
    reader: &mut PolicyContext<'_, Mode>,
) -> Result<(), PlanError>
where
    Mode: PolicyMode,
{
    let replaced_fee = removals.iter().try_fold(Capacity::zero(), |sum, hash| {
        let fee = accepted_entry(hash, reader)?.proof.metrics().fee;
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

fn accepted_entry<'authority, Mode>(
    hash: &RawTxHash,
    reader: &mut PolicyContext<'authority, Mode>,
) -> Result<Mode::Accepted<'authority>, PlanError>
where
    Mode: PolicyMode,
{
    reader.observe_accepted_owner(hash)
}
