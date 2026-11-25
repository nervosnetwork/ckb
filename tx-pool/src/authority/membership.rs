//! Membership decisions executed directly against `AuthorityState`.
//!
//! Expensive payload compaction and base-charge calculation run off-lock;
//! RBF, producer ancestry, late parents, unified capacity eviction, optional
//! replacement history, effects and mutation then settle in one atomic write cut.

use super::{
    budget::{Amount, Limits, compute_accepted_base_amount},
    eviction::{self, EvictionRank},
    ingress,
    jobs::Verified,
    model::{Accepted, Entry, Error, OutPointKey, Phase, RecoveryTriggers, Source, hash_key},
    notice::{Batch, Class, Effect},
    residency,
    state::{AuthorityState, FastHashMap, FastHashSet, OwnerEdit},
    store::Store,
};
use crate::{
    component::entry::TxEntrySnapshot, constants::MAX_POOL_MUTATION_CANDIDATES, error::Reject,
    util::compact_packed,
};
use ckb_app_config::TxPoolConfig;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{Capacity, cell::ResolvedTransaction, error::OutPointError},
    packed::{Byte32, OutPoint},
};
use ckb_verification::transaction_depends_on_time;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Aggregate {
    pub(super) count: usize,
    pub(super) bytes: usize,
    pub(super) cycles: u64,
    pub(super) fee: u128,
}

/// Both neighborhoods include the accepted owner itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct NeighborhoodTotals {
    pub(super) ancestors: Aggregate,
    pub(super) descendants: Aggregate,
}

fn overflow() -> Error {
    Reject::Full("accepted pool aggregate capacity overflow".into()).into()
}

impl Aggregate {
    pub(super) fn one(entry: &Accepted) -> Self {
        Self {
            count: 1,
            bytes: entry.size,
            cycles: entry.scripts.cycles(),
            fee: u128::from(entry.fee.as_u64()),
        }
    }

    pub(super) fn add(self, rhs: Self) -> Result<Self, Error> {
        Ok(Self {
            count: self.count.checked_add(rhs.count).ok_or_else(overflow)?,
            bytes: self.bytes.checked_add(rhs.bytes).ok_or_else(overflow)?,
            cycles: self.cycles.checked_add(rhs.cycles).ok_or_else(overflow)?,
            fee: self.fee.checked_add(rhs.fee).ok_or_else(overflow)?,
        })
    }

    pub(super) fn checked_sub(self, rhs: Self) -> Result<Self, Error> {
        let underflow = || Error::Fault("descendant aggregate underflow");
        Ok(Self {
            count: self.count.checked_sub(rhs.count).ok_or_else(underflow)?,
            bytes: self.bytes.checked_sub(rhs.bytes).ok_or_else(underflow)?,
            cycles: self.cycles.checked_sub(rhs.cycles).ok_or_else(underflow)?,
            fee: self.fee.checked_sub(rhs.fee).ok_or_else(underflow)?,
        })
    }

    pub(super) fn fee(self) -> Capacity {
        Capacity::shannons(self.fee.min(u128::from(u64::MAX)) as u64)
    }
}

fn accepted(entry: &Entry) -> Result<&Accepted, Error> {
    entry.accepted().ok_or(Error::Stale)
}

fn component_limit() -> Error {
    Reject::Full(format!(
        "pool mutation exceeds the per-transition limit of {MAX_POOL_MUTATION_CANDIDATES}"
    ))
    .into()
}

fn updated_usage(usage: Amount, released: Amount, added: Amount) -> Result<Amount, Error> {
    usage
        .checked_sub(released)
        .ok_or(Error::Stale)?
        .checked_add(added)
        .ok_or_else(overflow)
}

fn causal_cycle(hash: &Byte32) -> Error {
    Reject::Invalidated(format!(
        "candidate would create a causal cycle through {hash:?}"
    ))
    .into()
}

/// Walk one complete ancestry against either a captured map or `AuthorityState`.
pub(super) fn walk_ancestors<'a>(
    root: &'a Entry,
    limit: usize,
    lookup: impl Fn(&Byte32) -> Option<&'a Entry>,
    mut visit: impl FnMut(&Accepted) -> Result<(), Error>,
) -> Result<BTreeSet<Byte32>, Error> {
    let hash = root.transaction.hash_ref();
    let mut seen = BTreeSet::from([hash.clone()]);
    let root = accepted(root)?;
    visit(root)?;
    let mut stack: Vec<_> = root.parents.iter().collect();
    while let Some(parent) = stack.pop() {
        if parent == hash {
            return Err(causal_cycle(hash));
        }
        if !seen.insert(parent.clone()) {
            continue;
        }
        if seen.len() > limit {
            return Err(Reject::ExceededMaximumAncestorsCount.into());
        }
        let value = accepted(lookup(parent).ok_or(Error::Stale)?)?;
        visit(value)?;
        stack.extend(&value.parents);
    }
    if seen.len() > limit {
        return Err(Reject::ExceededMaximumAncestorsCount.into());
    }
    Ok(seen)
}

/// Visit each owner with its distinct ancestors, including itself.
#[expect(
    clippy::indexing_slicing,
    reason = "Positions, traversal marks and totals use ordinals from the same entry slice."
)]
pub(super) fn visit_ancestors(
    entries: &[&Entry],
    max_ancestors: usize,
    mut visit: impl FnMut(usize, &[usize], Aggregate) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut traversal = None;
    let mut ancestors = Vec::new();
    let mut stack = Vec::new();
    for (root, entry) in entries.iter().enumerate() {
        let hash = entry.transaction.hash_ref();
        let own = accepted(entry)?;
        if own.parents.is_empty() {
            if max_ancestors == 0 {
                return Err(Reject::ExceededMaximumAncestorsCount.into());
            }
            visit(root, std::slice::from_ref(&root), Aggregate::one(own))?;
            continue;
        }
        let (positions, seen) = traversal.get_or_insert_with(|| {
            let positions: HashMap<_, _> = entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.transaction.hash_ref(), index))
                .collect();
            (positions, vec![usize::MAX; entries.len()])
        });
        seen[root] = root;
        ancestors.clear();
        ancestors.push(root);
        stack.extend(own.parents.iter());
        while let Some(parent) = stack.pop() {
            if parent == hash {
                return Err(causal_cycle(hash));
            }
            let index = positions.get(parent).copied();
            if index.is_some_and(|index| seen[index] == root) {
                continue;
            }
            if ancestors.len() >= max_ancestors {
                return Err(Reject::ExceededMaximumAncestorsCount.into());
            }
            let index = index.ok_or(Error::Stale)?;
            seen[index] = root;
            ancestors.push(index);
            stack.extend(accepted(entries[index])?.parents.iter());
        }
        if ancestors.len() > max_ancestors {
            return Err(Reject::ExceededMaximumAncestorsCount.into());
        }
        let sum = ancestors
            .iter()
            .try_fold(Aggregate::default(), |sum, index| {
                sum.add(Aggregate::one(accepted(entries[*index])?))
            })?;
        visit(root, &ancestors, sum)?;
    }
    Ok(())
}

/// Both neighborhoods, keyed for callers that look up members after evaluation.
#[expect(
    clippy::indexing_slicing,
    reason = "The visitor supplies ordinals from entries."
)]
pub(super) fn aggregates<'a>(
    members: impl Iterator<Item = &'a Arc<Entry>>,
    max_ancestors: usize,
) -> Result<HashMap<[u8; 32], NeighborhoodTotals>, Error> {
    let entries: Vec<_> = members.map(Arc::as_ref).collect();
    let mut totals = vec![NeighborhoodTotals::default(); entries.len()];
    visit_ancestors(&entries, max_ancestors, |root, ancestors, sum| {
        totals[root].ancestors = sum;
        let own = Aggregate::one(accepted(entries[root])?);
        for &index in ancestors {
            totals[index].descendants = totals[index].descendants.add(own)?;
        }
        Ok(())
    })?;
    Ok(entries
        .into_iter()
        .map(Entry::hash_bytes)
        .zip(totals)
        .collect())
}

pub(super) fn entry_snapshot(
    entry: &Entry,
    totals: NeighborhoodTotals,
) -> Result<TxEntrySnapshot, Error> {
    let accepted = accepted(entry)?;
    let NeighborhoodTotals {
        ancestors,
        descendants,
    } = totals;
    let ancestors_fee = u64::try_from(ancestors.fee)
        .map(Capacity::shannons)
        .map_err(|_| overflow())?;
    Ok(TxEntrySnapshot {
        transaction: entry.transaction.as_ref().clone(),
        cycles: accepted.scripts.cycles(),
        size: accepted.size,
        fee: accepted.fee,
        ancestors_size: ancestors.bytes,
        ancestors_fee,
        ancestors_cycles: ancestors.cycles,
        ancestors_count: ancestors.count,
        descendants_fee: descendants.fee(),
        descendants_size: descendants.bytes,
        descendants_cycles: descendants.cycles,
        descendants_count: descendants.count,
        timestamp: accepted.timestamp,
    })
}

fn walk_ancestor_keys<'a>(
    roots: impl IntoIterator<Item = [u8; 32]>,
    excluded: &HashSet<[u8; 32]>,
    limit: usize,
    parents_of: impl Fn(&[u8; 32]) -> Result<&'a BTreeSet<Byte32>, Error>,
) -> Result<HashSet<[u8; 32]>, Error> {
    let mut result = HashSet::new();
    let mut stack: Vec<[u8; 32]> = roots.into_iter().collect();
    while let Some(hash) = stack.pop() {
        if excluded.contains(&hash) || !result.insert(hash) {
            continue;
        }
        if result.len() > limit {
            return Err(Reject::ExceededMaximumAncestorsCount.into());
        }
        for parent in parents_of(&hash)? {
            stack.push(hash_key(parent));
        }
    }
    Ok(result)
}

fn state_ancestors(
    state: &AuthorityState,
    roots: impl IntoIterator<Item = [u8; 32]>,
    excluded: &HashSet<[u8; 32]>,
    limit: usize,
) -> Result<HashSet<[u8; 32]>, Error> {
    walk_ancestor_keys(roots, excluded, limit, |hash| {
        Ok(&accepted(state.owner(hash).ok_or(Error::Stale)?)?.parents)
    })
}

fn rbf(
    state: &AuthorityState,
    candidate: &Entry,
    verified: &Verified,
    config: &TxPoolConfig,
) -> Result<HashSet<[u8; 32]>, Error> {
    let mut direct = HashSet::new();
    let mut conflict_point = None;
    for point in candidate.transaction.input_pts_iter() {
        if let Some(hash) = state.spenders.get(&OutPointKey::from_out_point(&point)) {
            direct.insert(*hash);
            conflict_point.get_or_insert(point);
        }
    }
    if direct.is_empty() {
        return Ok(HashSet::new());
    }
    if config.min_rbf_rate <= config.min_fee_rate {
        return Err(
            Reject::Resolve(OutPointError::Dead(conflict_point.ok_or(Error::Stale)?)).into(),
        );
    }
    let mut victim_inputs = HashSet::new();
    for hash in &direct {
        victim_inputs.extend(
            state
                .owner(hash)
                .ok_or(Error::Stale)?
                .transaction
                .input_pts_iter(),
        );
    }
    for point in candidate.transaction.input_pts_iter() {
        if verified.resolved().pool_cells.contains(&point) && !victim_inputs.contains(&point) {
            return Err(Reject::RBFRejected("new Tx contains unconfirmed inputs".into()).into());
        }
    }
    let empty = HashSet::new();
    let Some(removed) =
        state.descendants_of_roots(direct.iter().copied(), &empty, MAX_POOL_MUTATION_CANDIDATES)?
    else {
        return Err(Reject::RBFRejected(format!(
            "Tx conflict with too many txs, conflict txs count: {}, expect <= {} (count is a lower bound)",
            MAX_POOL_MUTATION_CANDIDATES.saturating_add(1),
            MAX_POOL_MUTATION_CANDIDATES
        ))
        .into());
    };
    let descendants: HashSet<[u8; 32]> = removed.difference(&direct).copied().collect();
    let mut producers = HashSet::new();
    for point in candidate.transaction.input_pts_iter() {
        let producer = hash_key(&point.tx_hash());
        if descendants.contains(&producer) {
            return Err(Reject::RBFRejected(
                "new Tx contains inputs in descendants of to be replaced Tx".into(),
            )
            .into());
        }
        if state
            .owner(&producer)
            .is_some_and(|e| e.accepted().is_some())
        {
            producers.insert(producer);
        }
    }
    let ancestor_limit = producers
        .len()
        .checked_mul(config.max_ancestors_count)
        .ok_or_else(overflow)?;
    if !state_ancestors(state, producers, &empty, ancestor_limit)?.is_disjoint(&descendants) {
        return Err(Reject::RBFRejected(
            "Tx ancestors have common with conflict Tx descendants".into(),
        )
        .into());
    }
    for point in verified.resolved().transaction.related_dep_out_points() {
        if removed.contains(&hash_key(&point.tx_hash())) {
            return Err(
                Reject::RBFRejected("new Tx contains cell deps from conflicts".into()).into(),
            );
        }
    }
    let fee_error = || {
        Error::Rejected(Reject::RBFRejected(
            "calculate_min_replace_fee failed".into(),
        ))
    };
    let total = removed.iter().try_fold(Capacity::zero(), |sum, hash| {
        sum.safe_add(accepted(state.owner(hash).ok_or(Error::Stale)?)?.fee)
            .map_err(|_| fee_error())
    })?;
    let required = total
        .safe_add(
            config
                .min_rbf_rate
                .fee(candidate.transaction.data().serialized_size_in_block() as u64),
        )
        .map_err(|_| fee_error())?;
    if verified.resolved().fee < required {
        return Err(Reject::RBFRejected(format!(
            "Tx's current fee is {}, expect it to >= {required} to replace old txs",
            verified.resolved().fee
        ))
        .into());
    }
    Ok(removed)
}

fn validate_backing(verified: &Verified, removed: &HashSet<[u8; 32]>) -> Result<(), Error> {
    let resolved = verified.resolved();
    if resolved.premises.has_spent()
        && let Some((point, _)) = resolved
            .premises
            .spenders
            .iter()
            .find(|(_, spender)| spender.is_some_and(|s| !removed.contains(&s)))
    {
        return Err(Reject::Resolve(OutPointError::Dead(point.to_out_point())).into());
    }
    if !removed.is_empty() {
        for point in &resolved.pool_cells {
            if removed.contains(&hash_key(&point.tx_hash())) {
                return Err(Reject::Resolve(OutPointError::Dead(point.clone())).into());
            }
        }
    }
    Ok(())
}

/// Descendant-before-parent topological order over `removed`, breaking ties by
/// smallest full hash so removal callbacks remain deterministic.
pub(super) fn removal_order(
    state: &AuthorityState,
    removed: &HashSet<[u8; 32]>,
) -> Result<Vec<[u8; 32]>, Error> {
    if removed.len() <= 1 {
        return Ok(removed.iter().copied().collect());
    }
    let mut counts: FastHashMap<[u8; 32], usize> =
        FastHashMap::with_capacity_and_hasher(removed.len(), Default::default());
    for hash in removed {
        counts.insert(*hash, 0);
    }
    for hash in removed {
        let entry = state.owner(hash).ok_or(Error::Stale)?;
        for parent in &accepted(entry)?.parents {
            if let Some(count) = counts.get_mut(&hash_key(parent)) {
                *count = count.checked_add(1).ok_or_else(overflow)?;
            }
        }
    }
    let mut leaves: BTreeSet<[u8; 32]> = counts
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(hash, _)| *hash)
        .collect();
    let mut order = Vec::with_capacity(removed.len());
    while let Some(hash) = leaves.pop_first() {
        let entry = state.owner(&hash).ok_or(Error::Stale)?;
        for parent in &accepted(entry)?.parents {
            let parent_key = hash_key(parent);
            if let Some(count) = counts.get_mut(&parent_key) {
                *count = count.checked_sub(1).ok_or_else(overflow)?;
                if *count == 0 {
                    leaves.insert(parent_key);
                }
            }
        }
        order.push(hash);
    }
    if order.len() != removed.len() {
        return Err(Error::Fault("accepted parent cycle"));
    }
    Ok(order)
}

fn build_history_entry(
    state: &AuthorityState,
    old: &Entry,
    candidate_inputs: &FastHashSet<OutPointKey>,
    removed: &HashSet<[u8; 32]>,
) -> Result<Option<Arc<Entry>>, Error> {
    let accepted = accepted(old)?;
    let mut keys = BTreeSet::new();
    for cell in accepted
        .transaction
        .resolved_inputs
        .iter()
        .chain(&accepted.transaction.resolved_cell_deps)
        .chain(&accepted.transaction.resolved_dep_groups)
    {
        let key = OutPointKey::from_out_point(&cell.out_point);
        let spent_by_survivor = state
            .spenders
            .get(&key)
            .is_some_and(|spender| !removed.contains(spender));
        if candidate_inputs.contains(&key)
            || spent_by_survivor
            || (cell.transaction_info.is_none()
                && removed.contains(&hash_key(&cell.out_point.tx_hash())))
        {
            keys.insert(compact_packed(&cell.out_point));
        }
    }
    let triggers = if keys.is_empty() {
        keys.extend(old.transaction.input_pts_iter().map(|p| compact_packed(&p)));
        RecoveryTriggers::RetryInputs(keys)
    } else {
        RecoveryTriggers::BlockedDependencies(keys)
    };
    if triggers.keys().is_empty() {
        return Ok(None);
    }
    Ok(Some(Entry::try_new(
        Arc::clone(&old.transaction),
        old.arrival,
        Source::Recovery,
        Phase::Replaced(triggers),
    )?))
}

#[derive(Clone, Copy)]
pub(super) struct AdmissionOptions {
    pub(super) retain_history: bool,
    pub(super) dry_run: bool,
    #[cfg(any(test, feature = "internal"))]
    pub(super) silence_effects: bool,
    #[cfg(any(test, feature = "internal"))]
    pub(super) forbid_displacement: bool,
}

impl AdmissionOptions {
    pub(super) fn commit() -> Self {
        Self {
            retain_history: true,
            dry_run: false,
            #[cfg(any(test, feature = "internal"))]
            silence_effects: false,
            #[cfg(any(test, feature = "internal"))]
            forbid_displacement: false,
        }
    }

    pub(super) fn preview() -> Self {
        Self {
            retain_history: false,
            dry_run: true,
            #[cfg(any(test, feature = "internal"))]
            silence_effects: false,
            #[cfg(any(test, feature = "internal"))]
            forbid_displacement: false,
        }
    }

    fn emit_effects(self) -> bool {
        #[cfg(any(test, feature = "internal"))]
        if self.silence_effects {
            return false;
        }
        !self.dry_run
    }
}

struct PreparedAdmission {
    admitted: Arc<Entry>,
    candidate_ancestors: HashSet<[u8; 32]>,
    replaced: HashSet<[u8; 32]>,
    removed: HashSet<[u8; 32]>,
    late: HashSet<[u8; 32]>,
    late_entries: HashMap<[u8; 32], Arc<Entry>>,
    late_descendants: HashSet<[u8; 32]>,
}

/// Evaluate RBF, backing, ancestry, late parents and capacity eviction directly
/// against `state` under the single-writer lock.
#[expect(clippy::too_many_arguments)]
fn evaluate_admission(
    state: &AuthorityState,
    limits: &super::budget::Limits,
    candidate: &Arc<Entry>,
    before: Option<&Arc<Entry>>,
    verified: &Verified,
    accepted_rtx: Arc<ckb_types::core::cell::ResolvedTransaction>,
    base_charge: Amount,
    context_sensitive: bool,
    config: &TxPoolConfig,
    options: AdmissionOptions,
) -> Result<PreparedAdmission, Error> {
    let candidate_hash = candidate.hash_bytes();
    let candidate_hash_b32 = candidate.hash();
    let replaced = rbf(state, candidate, verified, config)?;
    let mut removed = replaced.clone();
    validate_backing(verified, &removed)?;

    let mut parents = BTreeSet::new();
    let mut parent_keys = Vec::new();
    let mut consider_parent = |point: &OutPoint| {
        let producer = *OutPointKey::from_out_point(point).tx_hash();
        if !removed.contains(&producer)
            && state
                .owner(&producer)
                .is_some_and(|e| e.accepted().is_some())
            && parents.insert(Byte32::new(producer))
        {
            parent_keys.push(producer);
        }
    };
    for cell in &verified.resolved().transaction.resolved_inputs {
        consider_parent(&cell.out_point);
    }
    for point in verified.resolved().transaction.related_dep_out_points() {
        consider_parent(point);
    }
    let candidate_ancestors = if parent_keys.is_empty() {
        HashSet::new()
    } else {
        state_ancestors(
            state,
            parent_keys,
            &removed,
            config.max_ancestors_count.saturating_sub(1),
        )?
    };

    let mut late = HashSet::new();
    let mut readers_buf = Vec::new();
    for idx in 0..candidate.transaction.outputs().len() {
        readers_buf.clear();
        state.accepted_readers(
            &OutPointKey::from_parts(candidate_hash, idx as u32),
            &mut readers_buf,
        );
        late.extend(readers_buf.iter().copied().filter(|r| !removed.contains(r)));
    }
    let late_descendants = if late.is_empty() {
        HashSet::new()
    } else {
        let Some(descendants) = state.descendants_of_roots(
            late.iter().copied(),
            &removed,
            MAX_POOL_MUTATION_CANDIDATES.saturating_sub(removed.len()),
        )?
        else {
            return Err(component_limit());
        };
        if descendants.contains(&candidate_hash) {
            return Err(causal_cycle(&candidate_hash_b32));
        }
        if let Some(cycle) = descendants.intersection(&candidate_ancestors).next() {
            return Err(causal_cycle(&Byte32::from(cycle)));
        }
        descendants
    };

    let admitted = candidate.try_with_accepted(
        Accepted {
            transaction: accepted_rtx,
            scripts: verified.scripts(),
            fee: verified.resolved().fee,
            size: verified.serialized_size(),
            timestamp: verified.timestamp(),
            parents,
            context_sensitive,
            #[cfg(any(test, feature = "internal"))]
            forced_status: verified.forced_status(),
        },
        base_charge,
    )?;
    if candidate_ancestors.len().saturating_add(1) > config.max_ancestors_count {
        return Err(Reject::ExceededMaximumAncestorsCount.into());
    }

    let mut late_entries = HashMap::with_capacity(late.len());
    if !late.is_empty() {
        for hash in &late {
            let old = state.owner(hash).ok_or(Error::Stale)?;
            let mut updated = accepted(old)?.clone();
            updated.parents.insert(compact_packed(&candidate_hash_b32));
            late_entries.insert(*hash, old.try_with_phase(Phase::Accepted(updated))?);
        }
        let mut ordered_late: Vec<[u8; 32]> = late_descendants.iter().copied().collect();
        ordered_late.sort_unstable();
        for hash in ordered_late {
            walk_ancestor_keys([hash], &removed, config.max_ancestors_count, |current| {
                let entry = if *current == candidate_hash {
                    admitted.as_ref()
                } else if let Some(updated) = late_entries.get(current) {
                    updated.as_ref()
                } else {
                    state.owner(current).ok_or(Error::Stale)?.as_ref()
                };
                Ok(&accepted(entry)?.parents)
            })?;
        }
    }

    let mut released = Amount::default();
    for hash in &removed {
        released = released
            .checked_add(state.owner(hash).ok_or(Error::Stale)?.charge())
            .ok_or_else(overflow)?;
    }
    if !removed.contains(&candidate_hash)
        && let Some(prev) = state
            .owner(&candidate_hash)
            .filter(|e| e.accepted().is_some())
    {
        released = released.checked_add(prev.charge()).ok_or_else(overflow)?;
    }
    let mut added = admitted.charge();
    for (hash, updated) in &late_entries {
        released = released
            .checked_add(state.owner(hash).ok_or(Error::Stale)?.charge())
            .ok_or_else(overflow)?;
        added = added.checked_add(updated.charge()).ok_or_else(overflow)?;
    }

    let excess =
        updated_usage(state.usage.accepted, released, added)?.positive_difference(limits.accepted);
    if excess != Amount::default() {
        eviction::select_victims(
            state,
            &admitted,
            &candidate_ancestors,
            &late_entries,
            &late_descendants,
            &mut removed,
            excess,
        )?;
        validate_backing(verified, &removed)?;
    }

    #[cfg(any(test, feature = "internal"))]
    if options.forbid_displacement && (!removed.is_empty() || before.is_some()) {
        return Err(Error::Rejected(Reject::Full(
            "internal insertion would displace an owner".into(),
        )));
    }
    let _ = (before, options);

    Ok(PreparedAdmission {
        admitted,
        candidate_ancestors,
        replaced,
        removed,
        late,
        late_entries,
        late_descendants,
    })
}

fn sum_owners<'a>(
    state: &AuthorityState,
    initial: Aggregate,
    mut hashes: impl Iterator<Item = &'a [u8; 32]>,
) -> Result<Aggregate, Error> {
    hashes.try_fold(initial, |sum, hash| {
        sum.add(Aggregate::one(accepted(
            state.owner(hash).ok_or(Error::Stale)?,
        )?))
    })
}

fn single_owner_totals(
    state: &AuthorityState,
    hash: &[u8; 32],
    entry: &Entry,
    max_ancestors: usize,
) -> Result<NeighborhoodTotals, Error> {
    let own = accepted(entry)?;
    let ancestors = if own.parents.is_empty() {
        Aggregate::one(own)
    } else {
        let mut ancestors = Aggregate::default();
        walk_ancestors(
            entry,
            max_ancestors,
            |parent| state.owner(&hash_key(parent)).map(Arc::as_ref),
            |value| {
                ancestors = ancestors.add(Aggregate::one(value))?;
                Ok(())
            },
        )?;
        ancestors
    };
    let descendants = state.descendant_aggregate(hash)?;
    Ok(NeighborhoodTotals {
        ancestors,
        descendants,
    })
}

fn removal_snapshot(
    state: &AuthorityState,
    hash: &[u8; 32],
    old: &Entry,
    batch_totals: Option<&FastHashMap<[u8; 32], NeighborhoodTotals>>,
    max_ancestors: usize,
) -> Result<TxEntrySnapshot, Error> {
    let totals = match batch_totals {
        Some(totals) => *totals.get(hash).ok_or(Error::Stale)?,
        None => single_owner_totals(state, hash, old, max_ancestors)?,
    };
    entry_snapshot(old, totals)
}

fn accepted_effect(
    admitted: &Entry,
    compact_hash: Byte32,
    ancestors: Aggregate,
    descendants: Aggregate,
    snapshot: &Snapshot,
    options: AdmissionOptions,
) -> Result<Option<Effect>, Error> {
    if !options.emit_effects() {
        return Ok(None);
    }
    let status = accepted(admitted)?.status(snapshot);
    let entry_snap = entry_snapshot(
        admitted,
        NeighborhoodTotals {
            ancestors,
            descendants,
        },
    )?;
    Ok(Some(Effect::accepted_with_hash(
        entry_snap,
        compact_hash,
        status,
        admitted.source.residency_peer(),
    )))
}

/// Decide optional replacement history in a single pass against live usage.
fn collect_replacement_history(
    state: &AuthorityState,
    limits: &Limits,
    admitted: &Entry,
    released_pipeline: Amount,
    order: &[[u8; 32]],
    removed: &HashSet<[u8; 32]>,
    options: AdmissionOptions,
) -> Result<FastHashMap<[u8; 32], Arc<Entry>>, Error> {
    let mut history_entries = FastHashMap::default();
    if !options.retain_history
        || order.is_empty()
        || options.dry_run
        || state.usage.history.items.saturating_add(order.len()) > limits.history.items
    {
        return Ok(history_entries);
    }
    let candidate_inputs: FastHashSet<OutPointKey> = admitted
        .transaction
        .input_pts_iter()
        .map(|point| OutPointKey::from_out_point(&point))
        .collect();
    let mut total_charge = Amount::default();
    let mut built = Vec::with_capacity(order.len());
    for hash in order {
        let old = state.owner(hash).ok_or(Error::Stale)?;
        if let Some(history) = build_history_entry(state, old, &candidate_inputs, removed)? {
            let Some(next) = total_charge.checked_add(history.charge()) else {
                return Ok(history_entries);
            };
            total_charge = next;
            built.push((*hash, history));
        }
    }
    if state
        .usage
        .history_fits(total_charge, released_pipeline, limits)
    {
        history_entries.extend(built);
    }
    Ok(history_entries)
}

enum PlannedAdmission {
    Rejected(Reject),
    Single(OwnerEdit, Vec<Effect>),
    Multi(Vec<OwnerEdit>, Vec<Effect>),
}

#[allow(clippy::too_many_arguments)]
fn plan_multi_edit_admission(
    state: &AuthorityState,
    limits: &Limits,
    candidate_edit: OwnerEdit,
    candidate_effect: Option<Effect>,
    before_pipeline_charge: Amount,
    prepared: PreparedAdmission,
    config: &TxPoolConfig,
    options: AdmissionOptions,
) -> Result<(Vec<OwnerEdit>, Vec<Effect>), Error> {
    let PreparedAdmission {
        admitted,
        replaced,
        removed,
        late,
        late_entries,
        ..
    } = prepared;
    let order = removal_order(state, &removed)?;
    let removed_totals = (options.emit_effects() && order.len() > 1)
        .then(|| state.removal_totals(&order, config.max_ancestors_count))
        .transpose()?;
    let mut history_entries = collect_replacement_history(
        state,
        limits,
        &admitted,
        before_pipeline_charge,
        &order,
        &removed,
        options,
    )?;

    let mut effects = if options.emit_effects() {
        Vec::with_capacity(order.len().saturating_add(1))
    } else {
        Vec::new()
    };
    let mut edits = Vec::with_capacity(order.len().saturating_add(late.len()).saturating_add(1));

    for hash in &order {
        let old = state.owner(hash).cloned().ok_or(Error::Stale)?;
        if options.emit_effects() {
            let old_snapshot = removal_snapshot(
                state,
                hash,
                &old,
                removed_totals.as_ref(),
                config.max_ancestors_count,
            )?;
            let reason = if replaced.contains(hash) {
                Reject::RBFRejected(format!("replaced by tx {}", admitted.hash()))
            } else {
                let rank = EvictionRank::for_entry(
                    &old,
                    Aggregate::one(accepted(&old)?),
                    &state.snapshot,
                )?;
                Reject::Full(format!(
                    "the fee_rate for this transaction is: {}",
                    rank.fee
                ))
            };
            effects.push(Effect::removed(&old, reason, Some(old_snapshot))?);
        }
        edits.push(OwnerEdit::new(
            *hash,
            Some(old),
            history_entries.remove(hash),
        ));
    }
    for hash in &late {
        if !removed.contains(hash) {
            let old = state.owner(hash).cloned().ok_or(Error::Stale)?;
            let after = late_entries.get(hash).cloned().ok_or(Error::Stale)?;
            edits.push(OwnerEdit::new(*hash, Some(old), Some(after)));
        }
    }
    effects.extend(candidate_effect);
    edits.push(candidate_edit);
    Ok((edits, effects))
}

#[allow(clippy::too_many_arguments)]
fn plan_admission_transition(
    state: &AuthorityState,
    limits: &Limits,
    candidate: &Arc<Entry>,
    before: Option<Arc<Entry>>,
    verified: &Verified,
    accepted_rtx: Arc<ResolvedTransaction>,
    base_charge: Amount,
    context_sensitive: bool,
    compact_hash: Byte32,
    config: &TxPoolConfig,
    options: AdmissionOptions,
) -> Result<PlannedAdmission, Error> {
    let resolved = verified.resolved();
    let candidate_hash = candidate.hash_bytes();
    state.validate_premises(resolved.view, &resolved.premises)?;
    let matches_before = match (&before, state.owner(&candidate_hash)) {
        (Some(expected), Some(actual)) => Arc::ptr_eq(expected, actual),
        (None, None) => true,
        _ => false,
    };
    if !matches_before {
        return Err(Error::Stale);
    }
    let prepared = match evaluate_admission(
        state,
        limits,
        candidate,
        before.as_ref(),
        verified,
        accepted_rtx,
        base_charge,
        context_sensitive,
        config,
        options,
    ) {
        Ok(prepared) => prepared,
        Err(Error::Rejected(reject)) => return Ok(PlannedAdmission::Rejected(reject)),
        Err(error) => return Err(error),
    };

    let own = Aggregate::one(accepted(&prepared.admitted)?);
    let ancestors_agg = sum_owners(state, own, prepared.candidate_ancestors.iter())?;
    let descendants_agg = sum_owners(
        state,
        own,
        prepared
            .late_descendants
            .iter()
            .filter(|h| !prepared.removed.contains(*h)),
    )?;
    let candidate_effect = accepted_effect(
        &prepared.admitted,
        compact_hash,
        ancestors_agg,
        descendants_agg,
        &state.snapshot,
        options,
    )?;
    let before_pipeline_charge = before
        .as_ref()
        .filter(|entry| entry.accepted().is_none())
        .map_or(Amount::default(), |entry| entry.charge());
    let candidate_edit = OwnerEdit {
        hash: candidate_hash,
        before,
        after: Some(Arc::clone(&prepared.admitted)),
        ancestors: Some(ancestors_agg),
    };

    if prepared.removed.is_empty() && prepared.late.is_empty() {
        return Ok(PlannedAdmission::Single(
            candidate_edit,
            candidate_effect.into_iter().collect(),
        ));
    }

    let (edits, effects) = plan_multi_edit_admission(
        state,
        limits,
        candidate_edit,
        candidate_effect,
        before_pipeline_charge,
        prepared,
        config,
        options,
    )?;
    Ok(PlannedAdmission::Multi(edits, effects))
}

/// Admit a verified candidate (or preview admission with `options.dry_run`).
///
/// Compacts resolved dependencies and precomputes the accepted base charge and
/// compact relay hash off-lock. Dry-run previews (`test_accept_tx`) validate
/// completely under `state.read()` without acquiring the single-writer lock;
/// commits validate premises, evaluate RBF/ancestry/eviction, reserve outbox
/// notices and commit atomically under `state.write()`.
pub(super) fn commit_admission(
    store: &Store,
    candidate: &Arc<Entry>,
    before: Option<Arc<Entry>>,
    verified: &Verified,
    config: &TxPoolConfig,
    options: AdmissionOptions,
) -> Result<(Option<Arc<Batch>>, Option<Reject>), Error> {
    #[cfg(test)]
    store
        .admission_attempts
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    #[cfg(feature = "profiling")]
    let _span = tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.membership.admission")
        .entered();

    let resolved = verified.resolved();
    let accepted_rtx = residency::accepted_resolution(&resolved.transaction);
    let base_charge = compute_accepted_base_amount(
        &candidate.transaction,
        &accepted_rtx,
        verified.serialized_size(),
        verified.cycles(),
    )?;
    let context_sensitive = transaction_depends_on_time(&resolved.transaction);
    let candidate_hash_b32 = candidate.hash();
    let compact_hash = compact_packed(&candidate_hash_b32);

    if options.dry_run {
        let reject = store.preview_with(|state, limits, now| {
            #[cfg(test)]
            store.run_preview_observer(&candidate_hash_b32);
            match plan_admission_transition(
                state,
                limits,
                candidate,
                before,
                verified,
                accepted_rtx,
                base_charge,
                context_sensitive,
                compact_hash,
                config,
                options,
            )? {
                PlannedAdmission::Rejected(reject) => Ok(Some(reject)),
                PlannedAdmission::Single(edit, _) => {
                    state.preflight_edits(std::slice::from_ref(&edit), limits, now, &[])?;
                    Ok(None)
                }
                PlannedAdmission::Multi(edits, _) => {
                    state.preflight_edits(&edits, limits, now, &[])?;
                    Ok(None)
                }
            }
        })?;
        return Ok((None, reject));
    }

    let (reject, batch) = store.commit_with(|tx| {
        let retire = before
            .as_ref()
            .filter(|entry| Arc::ptr_eq(entry, candidate))
            .cloned();
        match plan_admission_transition(
            tx.state,
            tx.limits,
            candidate,
            before,
            verified,
            accepted_rtx,
            base_charge,
            context_sensitive,
            compact_hash,
            config,
            options,
        )? {
            PlannedAdmission::Rejected(reject) => {
                if options.emit_effects() {
                    ingress::commit_rejection_locked(
                        tx,
                        retire,
                        &candidate_hash_b32,
                        candidate.source,
                        reject.clone(),
                    )?;
                }
                Ok(Some(reject))
            }
            PlannedAdmission::Single(edit, effects) => {
                tx.commit_single_with_ancestors(edit, effects, ingress::class(candidate.source))?;
                Ok(None)
            }
            PlannedAdmission::Multi(edits, effects) => {
                tx.commit_edits(edits, Vec::new(), effects, ingress::class(candidate.source))?;
                Ok(None)
            }
        }
    })?;
    Ok((batch, reject))
}

/// Remove a live owner (and its accepted descendants, leaf-first) atomically.
pub(super) fn commit_removal(
    store: &Store,
    root: &Arc<Entry>,
    config: &TxPoolConfig,
    reason: Option<Reject>,
) -> Result<Option<Arc<Batch>>, Error> {
    let root_hash = root.hash_bytes();
    let (_, batch) = store.commit_with(|tx| {
        if !tx
            .state
            .owner(&root_hash)
            .is_some_and(|current| Arc::ptr_eq(current, root))
        {
            return Err(Error::Stale);
        }
        if root.accepted().is_none() || !tx.state.children.contains_key(&root_hash) {
            let effects: Vec<Effect> = match (&reason, root.accepted()) {
                (Some(reason), Some(_)) => {
                    let snap = removal_snapshot(
                        tx.state,
                        &root_hash,
                        root,
                        None,
                        config.max_ancestors_count,
                    )?;
                    vec![Effect::removed(root, reason.clone(), Some(snap))?]
                }
                _ => Vec::new(),
            };
            return tx.commit_single(
                root_hash,
                Some(Arc::clone(root)),
                None,
                effects,
                Class::Trusted,
            );
        }
        let empty = HashSet::new();
        let removed = tx.state.descendants_unbounded(&root_hash, &empty)?;
        let mut order = removal_order(tx.state, &removed)?;
        if reason.is_some() {
            order.truncate(MAX_POOL_MUTATION_CANDIDATES);
        }
        let removed_totals = (reason.is_some() && order.len() > 1)
            .then(|| tx.state.removal_totals(&order, config.max_ancestors_count))
            .transpose()?;
        let mut effects = Vec::new();
        let mut edits = Vec::with_capacity(order.len());
        for hash in &order {
            let old = tx.state.owner(hash).cloned().ok_or(Error::Stale)?;
            if let Some(reason) = &reason {
                let snap = removal_snapshot(
                    tx.state,
                    hash,
                    &old,
                    removed_totals.as_ref(),
                    config.max_ancestors_count,
                )?;
                effects.push(Effect::removed(&old, reason.clone(), Some(snap))?);
            }
            edits.push(OwnerEdit::remove(*hash, old));
        }
        tx.commit_edits(edits, Vec::new(), effects, Class::Trusted)
    })?;
    Ok(batch)
}

#[cfg(test)]
#[path = "tests/membership_trim.rs"]
mod trim_tests;
