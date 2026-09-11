//! Membership decisions run over captured facts. RBF, producer ancestry, late
//! parents, capacity victims and notices become one checked Store::apply.
use super::{
    budget::{Amount, owner_amount},
    ingress,
    jobs::{Verified, context_sensitive},
    model::{Accepted, DependencyKey, Entry, Error, Phase, Source, Status},
    notice::Effect,
    residency,
    store::{Plan, Store},
};
use crate::{
    component::entry::TxEntrySnapshot, constants::MAX_POOL_MUTATION_CANDIDATES, error::Reject,
    util::compact_packed,
};
use ckb_app_config::TxPoolConfig;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{Capacity, FeeRate, error::OutPointError, tx_pool::get_transaction_weight},
    packed::{Byte32, OutPoint},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

mod graph;
pub(super) use graph::Graph;

pub(super) type Members = BTreeMap<Byte32, Arc<Entry>>;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Aggregate {
    pub(super) count: usize,
    pub(super) bytes: usize,
    pub(super) cycles: u64,
    pub(super) fee: u128,
}
fn overflow() -> Error {
    Reject::Full("accepted pool aggregate capacity overflow".into()).into()
}
impl Aggregate {
    pub(super) fn one(entry: &Accepted) -> Self {
        Self {
            count: 1,
            bytes: entry.size,
            cycles: entry.cycles,
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
    fn sub(self, rhs: Self) -> Result<Self, Error> {
        Ok(Self {
            count: self.count.checked_sub(rhs.count).ok_or_else(overflow)?,
            bytes: self.bytes.checked_sub(rhs.bytes).ok_or_else(overflow)?,
            cycles: self.cycles.checked_sub(rhs.cycles).ok_or_else(overflow)?,
            fee: self.fee.checked_sub(rhs.fee).ok_or_else(overflow)?,
        })
    }
    pub(super) fn fee(self) -> Capacity {
        Capacity::shannons(self.fee.min(u128::from(u64::MAX)) as u64)
    }
}
fn accepted(entry: &Entry) -> Result<&Accepted, Error> {
    entry.accepted().ok_or(Error::Stale)
}
fn component_limit(rbf: bool) -> Error {
    if rbf {
        Reject::RBFRejected(format!(
            "Tx conflict with too many txs, conflict txs count: >= {}, expect <= {}",
            MAX_POOL_MUTATION_CANDIDATES + 1,
            MAX_POOL_MUTATION_CANDIDATES
        ))
        .into()
    } else {
        Reject::Full(format!(
            "pool mutation exceeds the per-transition limit of {MAX_POOL_MUTATION_CANDIDATES}"
        ))
        .into()
    }
}
fn causal_cycle(hash: &Byte32) -> Error {
    Reject::Invalidated(format!(
        "candidate would create a causal cycle through {hash:?}"
    ))
    .into()
}

pub(super) fn ancestor_hashes(
    members: &Members,
    hash: &Byte32,
    limit: usize,
) -> Result<BTreeSet<Byte32>, Error> {
    let entry = members.get(hash).ok_or(Error::Stale)?;
    let mut seen = BTreeSet::from([hash.clone()]);
    let mut stack: Vec<_> = accepted(entry)?.parents.iter().cloned().collect();
    while let Some(parent) = stack.pop() {
        if parent == *hash {
            return Err(causal_cycle(hash));
        }
        if !seen.insert(parent.clone()) {
            continue;
        }
        if seen.len() > limit {
            return Err(Reject::ExceededMaximumAncestorsCount.into());
        }
        let entry = members.get(&parent).ok_or(Error::Stale)?;
        stack.extend(accepted(entry)?.parents.iter().cloned());
    }
    if seen.len() > limit {
        return Err(Reject::ExceededMaximumAncestorsCount.into());
    }
    Ok(seen)
}
pub(super) fn children(members: &Members) -> BTreeMap<Byte32, BTreeSet<Byte32>> {
    let mut children: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
    for (hash, entry) in members {
        if let Some(entry) = entry.accepted() {
            for parent in &entry.parents {
                children
                    .entry(parent.clone())
                    .or_default()
                    .insert(hash.clone());
            }
        }
    }
    children
}
pub(super) fn descendant_hashes(
    children: &BTreeMap<Byte32, BTreeSet<Byte32>>,
    roots: impl IntoIterator<Item = Byte32>,
    limit: usize,
) -> Result<BTreeSet<Byte32>, Error> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<_> = roots.into_iter().collect();
    while let Some(hash) = stack.pop() {
        if !seen.insert(hash.clone()) {
            continue;
        }
        if seen.len() > limit {
            return Err(component_limit(false));
        }
        if let Some(children) = children.get(&hash) {
            stack.extend(children.iter().cloned());
        }
    }
    Ok(seen)
}
pub(super) fn aggregate(members: &Members, hashes: &BTreeSet<Byte32>) -> Result<Aggregate, Error> {
    hashes.iter().try_fold(Aggregate::default(), |sum, hash| {
        sum.add(Aggregate::one(accepted(
            members.get(hash).ok_or(Error::Stale)?,
        )?))
    })
}
/// Recompute each member's bounded ancestor closure and descendant totals.
/// Reuse ordinal marks and scratch across roots; no transitive sets or owner
/// references survive this calculation. The returned map remains the only totals.
#[expect(
    clippy::indexing_slicing,
    reason = "Ordinals come from the same sorted Members keys as the entries, marks and mutable totals rows."
)]
pub(super) fn aggregates(
    members: &Members,
    max_ancestors: usize,
) -> Result<BTreeMap<Byte32, (Aggregate, Aggregate)>, Error> {
    let mut totals: BTreeMap<_, _> = members
        .keys()
        .map(|hash| (hash.clone(), (Aggregate::default(), Aggregate::default())))
        .collect();
    let entries: Vec<_> = members.iter().collect();
    let positions: std::collections::HashMap<_, _> = members
        .keys()
        .enumerate()
        .map(|(index, hash)| (hash, index))
        .collect();
    let mut rows: Vec<_> = totals.values_mut().collect();
    let mut seen = vec![usize::MAX; entries.len()];
    let mut ancestors = Vec::new();
    let mut stack = Vec::new();
    for (root, &(hash, entry)) in entries.iter().enumerate() {
        let own = accepted(entry)?;
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
            // Check a new parent before loading it, matching ancestor_hashes:
            // even an absent parent first exceeds an already full closure.
            if ancestors.len() >= max_ancestors {
                return Err(Reject::ExceededMaximumAncestorsCount.into());
            }
            let index = index.ok_or(Error::Stale)?;
            seen[index] = root;
            ancestors.push(index);
            stack.extend(accepted(entries[index].1)?.parents.iter());
        }
        if ancestors.len() > max_ancestors {
            return Err(Reject::ExceededMaximumAncestorsCount.into());
        }
        rows[root].0 = ancestors
            .iter()
            .try_fold(Aggregate::default(), |sum, index| {
                sum.add(Aggregate::one(accepted(entries[*index].1)?))
            })?;
        let own = Aggregate::one(own);
        for index in &ancestors {
            rows[*index].1 = rows[*index].1.add(own)?;
        }
    }
    Ok(totals)
}

pub(super) fn snapshot(
    entry: &Entry,
    ancestors: Aggregate,
    descendants: Aggregate,
) -> Result<TxEntrySnapshot, Error> {
    let accepted = accepted(entry)?;
    let ancestors_fee = u64::try_from(ancestors.fee)
        .map(Capacity::shannons)
        .map_err(|_| overflow())?;
    Ok(TxEntrySnapshot {
        transaction: entry.transaction.as_ref().clone(),
        cycles: accepted.cycles,
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EvictionRank {
    status: Status,
    fee: FeeRate,
    descendants: usize,
    arrival: u64,
    hash: Byte32,
}
impl EvictionRank {
    fn new(entry: &Entry, descendants: Aggregate, snapshot: &Snapshot) -> Result<Self, Error> {
        let value = accepted(entry)?;
        let own_rate =
            FeeRate::calculate(value.fee, get_transaction_weight(value.size, value.cycles));
        let descendants_rate = FeeRate::calculate(
            descendants.fee(),
            get_transaction_weight(descendants.bytes, descendants.cycles),
        );
        Ok(Self {
            status: value.status(snapshot),
            fee: own_rate.max(descendants_rate),
            descendants: descendants.count,
            arrival: entry.arrival,
            hash: entry.hash(),
        })
    }
}
#[derive(Clone, Copy)]
enum Removal {
    Replacement,
    Capacity,
}
impl Removal {
    fn reject(self, old: &Entry, winner: &Byte32, snapshot: &Snapshot) -> Result<Reject, Error> {
        Ok(match self {
            Self::Replacement => Reject::RBFRejected(format!("replaced by tx {winner}")),
            Self::Capacity => Reject::Full(format!(
                "the fee_rate for this transaction is: {}",
                EvictionRank::new(old, Aggregate::one(accepted(old)?), snapshot)?.fee
            )),
        })
    }
}

fn rbf(
    graph: &mut Graph<'_>,
    candidate: &Entry,
    verified: &Verified,
    config: &TxPoolConfig,
) -> Result<BTreeSet<Byte32>, Error> {
    let mut direct = BTreeSet::new();
    let mut conflict_point = None;
    for point in candidate.transaction.input_pts_iter() {
        if let Some(hash) = graph.spender(&point)? {
            direct.insert(hash);
            conflict_point.get_or_insert(point);
        }
    }
    if direct.is_empty() {
        return Ok(BTreeSet::new());
    }
    if config.min_rbf_rate <= config.min_fee_rate {
        return Err(
            Reject::Resolve(OutPointError::Dead(conflict_point.ok_or(Error::Stale)?)).into(),
        );
    }
    let mut victim_inputs = BTreeSet::new();
    for hash in &direct {
        victim_inputs.extend(graph.require(hash)?.transaction.input_pts_iter());
    }
    for point in candidate.transaction.input_pts_iter() {
        if verified.resolved().pool_cells.contains(&point) && !victim_inputs.contains(&point) {
            return Err(Reject::RBFRejected("new Tx contains unconfirmed inputs".into()).into());
        }
    }
    let removed = graph
        .descendants(
            direct.iter().cloned(),
            &BTreeSet::new(),
            MAX_POOL_MUTATION_CANDIDATES,
        )
        .map_err(|error| {
            if matches!(error, Error::Rejected(Reject::Full(_))) {
                component_limit(true)
            } else {
                error
            }
        })?;
    let descendants: BTreeSet<_> = removed.difference(&direct).cloned().collect();
    let mut producers = BTreeSet::new();
    for point in candidate.transaction.input_pts_iter() {
        if descendants.contains(&point.tx_hash()) {
            return Err(Reject::RBFRejected(
                "new Tx contains inputs in descendants of to be replaced Tx".into(),
            )
            .into());
        }
        if graph.get(&point.tx_hash())?.is_some() {
            producers.insert(point.tx_hash());
        }
    }
    let ancestor_limit = producers
        .len()
        .checked_mul(config.max_ancestors_count)
        .ok_or_else(overflow)?;
    let ancestors = graph.ancestors(producers, &BTreeSet::new(), ancestor_limit)?;
    if !ancestors.is_disjoint(&descendants) {
        return Err(Reject::RBFRejected(
            "Tx ancestors have common with conflict Tx descendants".into(),
        )
        .into());
    }
    for point in verified.resolved().transaction.related_dep_out_points() {
        if removed.contains(&point.tx_hash()) {
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
        sum.safe_add(accepted(graph.require(hash)?.as_ref())?.fee)
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
fn validate_backing(
    verified: &Verified,
    inputs: &BTreeSet<OutPoint>,
    removed: &BTreeSet<Byte32>,
) -> Result<(), Error> {
    // Inputs require atomic replacement of their spender. A cell-dep reader
    // may precede an existing spender in a block; packing enforces that order.
    if let Some((point, _)) = verified
        .resolved()
        .reads
        .spent()
        .find(|(point, spender)| inputs.contains(*point) && !removed.contains(*spender))
    {
        return Err(Reject::Resolve(OutPointError::Dead(point.clone())).into());
    }
    if let Some(point) = verified
        .resolved()
        .pool_cells
        .iter()
        .find(|point| removed.contains(&point.tx_hash()))
    {
        return Err(Reject::Resolve(OutPointError::Dead(point.clone())).into());
    }
    Ok(())
}
fn candidate_parents(
    graph: &mut Graph<'_>,
    candidate: &Entry,
    verified: &Verified,
    removed: &BTreeSet<Byte32>,
) -> Result<BTreeSet<Byte32>, Error> {
    let mut parents = BTreeSet::new();
    for point in candidate.transaction.input_pts_iter().chain(
        verified
            .resolved()
            .transaction
            .related_dep_out_points()
            .cloned(),
    ) {
        if !removed.contains(&point.tx_hash()) && graph.get(&point.tx_hash())?.is_some() {
            parents.insert(compact_packed(&point.tx_hash()));
        }
    }
    // A reader is not a causal parent of the spender. Their relative order
    // matters only if both are selected into a block; packing derives that
    // conditional edge from the selected inputs and cell dependencies.
    Ok(parents)
}
fn apply_virtual(
    original: &Members,
    candidate: &Arc<Entry>,
    late: &BTreeSet<Byte32>,
    removed: &BTreeSet<Byte32>,
) -> Result<Members, Error> {
    let mut entries = original.clone();
    entries.retain(|hash, _| !removed.contains(hash));
    entries.insert(candidate.hash(), Arc::clone(candidate));
    for hash in late.difference(removed) {
        let old = entries.get(hash).ok_or(Error::Stale)?;
        let mut value = accepted(old)?.clone();
        value.parents.insert(candidate.hash());
        entries.insert(hash.clone(), old.with_phase(Phase::Accepted(value)));
    }
    Ok(entries)
}
fn total_charge(entries: &Members) -> Result<Amount, Error> {
    entries
        .values()
        .try_fold(Amount::default(), |total, entry| {
            total.checked_add(owner_amount(entry)?).ok_or_else(overflow)
        })
}

/// The same policy prepares both real admission and dry-run validation.
pub(super) fn admission(
    store: &Store,
    candidate: &Arc<Entry>,
    before: Option<Arc<Entry>>,
    verified: &Verified,
    config: &TxPoolConfig,
    retain_history: bool,
) -> Result<(Plan, Option<Reject>), Error> {
    #[cfg(test)]
    store
        .admission_attempts
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    #[cfg(feature = "profiling")]
    let _span = tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.membership.admission")
        .entered();
    let (view, snapshot) = store.snapshot();
    if view != verified.resolved().view {
        return Err(Error::Stale);
    }
    let mut plan = Plan::new(
        view,
        ingress::class(candidate.source),
        verified.resolved().reads.clone(),
    );
    plan.observe_owner(&candidate.hash(), before.as_ref())?;
    let outcome = prepare_admission(
        &mut Graph::new(store, &mut plan),
        &snapshot,
        candidate,
        before.clone(),
        verified,
        config,
        retain_history,
    );
    match outcome {
        Ok(()) => Ok((plan, None)),
        Err(Error::Rejected(reject)) => {
            let retire = before.filter(|entry| Arc::ptr_eq(entry, candidate));
            let plan = ingress::rejection(
                store,
                plan,
                retire,
                &candidate.hash(),
                candidate.source,
                reject.clone(),
            )?;
            Ok((plan, Some(reject)))
        }
        Err(error) => Err(error),
    }
}

fn prepare_admission(
    graph: &mut Graph<'_>,
    snapshot: &Snapshot,
    candidate: &Arc<Entry>,
    before: Option<Arc<Entry>>,
    verified: &Verified,
    config: &TxPoolConfig,
    retain_history: bool,
) -> Result<(), Error> {
    let mut removed = rbf(graph, candidate, verified, config)?;
    let mut causes: BTreeMap<_, _> = removed
        .iter()
        .map(|hash| (hash.clone(), Removal::Replacement))
        .collect();
    let inputs = candidate.transaction.input_pts_iter().collect();
    validate_backing(verified, &inputs, &removed)?;
    let parents = candidate_parents(graph, candidate, verified, &removed)?;
    let ancestors = graph.ancestors(
        parents.iter().cloned(),
        &removed,
        config.max_ancestors_count.saturating_sub(1),
    )?;
    let mut late = BTreeSet::new();
    for point in candidate.transaction.output_pts_iter() {
        for child in graph.readers(point)? {
            if !removed.contains(&child) {
                late.insert(child);
            }
        }
    }
    let late_descendants = graph.descendants(
        late.iter().cloned(),
        &removed,
        MAX_POOL_MUTATION_CANDIDATES.saturating_sub(removed.len()),
    )?;
    if let Some(hash) = late_descendants.intersection(&ancestors).next() {
        return Err(causal_cycle(hash));
    }
    for hash in &late_descendants {
        graph.ancestors([hash.clone()], &removed, config.max_ancestors_count)?;
    }
    let value = Accepted {
        transaction: residency::accepted_resolution(&verified.resolved().transaction),
        cycles: verified.cycles(),
        fee: verified.resolved().fee,
        size: verified.serialized_size(),
        timestamp: verified.timestamp(),
        parents,
        context_sensitive: context_sensitive(&verified.resolved().transaction),
        #[cfg(any(test, feature = "internal"))]
        forced_status: verified.forced_status(),
    };
    let admitted = candidate.with_phase(Phase::Accepted(value));
    let mut virtual_entries = apply_virtual(graph.entries(), &admitted, &late, &removed)?;
    let mut affected = late_descendants.clone();
    affected.insert(candidate.hash());
    for hash in &affected {
        ancestor_hashes(&virtual_entries, hash, config.max_ancestors_count)?;
    }
    let mut released = Amount::default();
    for hash in &removed {
        released = released
            .checked_add(owner_amount(graph.require(hash)?.as_ref())?)
            .ok_or_else(overflow)?;
    }
    let mut added = owner_amount(&admitted)?;
    // Existing late children gain only direct parent metadata. Count those
    // owner differences in the same reservation as candidate and all victims.
    for hash in &late {
        let old = owner_amount(graph.require(hash)?.as_ref())?;
        let new = owner_amount(virtual_entries.get(hash).ok_or(Error::Stale)?)?;
        released = released.checked_add(old).ok_or_else(overflow)?;
        added = added.checked_add(new).ok_or_else(overflow)?;
    }
    let optimistic = graph
        .accepted_usage()
        .checked_sub(released)
        .and_then(|usage| usage.checked_add(added));
    if optimistic.is_none_or(|usage| !usage.fits(graph.limits().accepted)) {
        // This speculative map will be rebuilt from the full cut below.
        // Release it before the full capture and replacement map overlap.
        drop(virtual_entries);
        graph.capture_accepted()?;
        virtual_entries = apply_virtual(graph.entries(), &admitted, &late, &removed)?;
        trim_virtual(
            &mut virtual_entries,
            snapshot,
            config,
            &mut removed,
            &mut causes,
            &late_descendants,
            &admitted.hash(),
            graph.limits().accepted,
        )?;
    }
    validate_backing(verified, &inputs, &removed)?;
    let order = removal_order(graph.entries(), &removed)?;
    let removed_totals = if order.len() > 1 {
        Some(graph.removal_totals(&order, config.max_ancestors_count)?)
    } else {
        None
    };
    for hash in order {
        let old = graph.require(&hash)?;
        let old_snapshot = if let Some(totals) = &removed_totals {
            let (ancestors, descendants) = totals.get(&hash).ok_or(Error::Stale)?;
            self::snapshot(&old, *ancestors, *descendants)?
        } else {
            graph.entry_snapshot(&hash, config.max_ancestors_count)?
        };
        let reason =
            causes
                .get(&hash)
                .ok_or(Error::Stale)?
                .reject(&old, &candidate.hash(), snapshot)?;
        let effect = Effect::rejected(&hash, reason, Some(old_snapshot), true)?;
        let history = if retain_history {
            history(&old, &inputs, &removed, graph)?
        } else {
            None
        };
        graph.plan.edit(Some(old), history, Some(effect))?;
    }
    for hash in late.difference(&removed) {
        let old = graph.require(hash)?;
        let after = virtual_entries.get(hash).cloned().ok_or(Error::Stale)?;
        graph.plan.edit(Some(old), Some(after), None)?;
    }
    let ancestors = ancestor_hashes(
        &virtual_entries,
        &admitted.hash(),
        config.max_ancestors_count,
    )?;
    let descendants = if late.is_empty() {
        Aggregate::one(accepted(&admitted)?)
    } else {
        let descendants = descendant_hashes(
            &children(&virtual_entries),
            [admitted.hash()],
            graph.limits().accepted.items,
        )?;
        aggregate(&virtual_entries, &descendants)?
    };
    let accepted_snapshot = self::snapshot(
        &admitted,
        aggregate(&virtual_entries, &ancestors)?,
        descendants,
    )?;
    let effect = Effect::accepted(
        accepted_snapshot,
        accepted(&admitted)?.status(snapshot),
        candidate.source.residency_peer(),
    );
    graph.plan.edit(before, Some(admitted), Some(effect))
}
#[expect(
    clippy::too_many_arguments,
    reason = "One bounded virtual membership operation; arguments are its existing inputs and outputs."
)]
fn trim_virtual(
    entries: &mut Members,
    snapshot: &Snapshot,
    config: &TxPoolConfig,
    removed: &mut BTreeSet<Byte32>,
    causes: &mut BTreeMap<Byte32, Removal>,
    late: &BTreeSet<Byte32>,
    candidate: &Byte32,
    limit: Amount,
) -> Result<(), Error> {
    let mut charge = total_charge(entries)?;
    if charge.fits(limit) {
        return Ok(());
    }
    let totals = aggregates(entries, config.max_ancestors_count)?;
    let mut descendants: BTreeMap<_, _> = totals
        .into_iter()
        .map(|(hash, (_, total))| (hash, total))
        .collect();
    let mut ranks = BTreeSet::new();
    for (hash, entry) in entries.iter() {
        ranks.insert(EvictionRank::new(
            entry,
            *descendants.get(hash).ok_or(Error::Stale)?,
            snapshot,
        )?);
    }
    let child_index = children(entries);
    let protected = ancestor_hashes(entries, candidate, config.max_ancestors_count)?;
    let candidate_rate = EvictionRank::new(
        entries.get(candidate).ok_or(Error::Stale)?,
        Aggregate::one(accepted(entries.get(candidate).ok_or(Error::Stale)?)?),
        snapshot,
    )?
    .fee;
    while !charge.fits(limit) {
        let root = ranks.first().ok_or_else(overflow)?.clone();
        let closure = descendant_hashes(&child_index, [root.hash], MAX_POOL_MUTATION_CANDIDATES)?;
        let closure: BTreeSet<_> = closure
            .into_iter()
            .filter(|hash| entries.contains_key(hash))
            .collect();
        if !closure.is_disjoint(&protected) {
            return Err(Reject::Full(format!(
                "the fee_rate for this transaction is: {candidate_rate}"
            ))
            .into());
        }
        let touched = removed
            .union(late)
            .cloned()
            .chain(closure.iter().cloned())
            .collect::<BTreeSet<_>>();
        if touched.len() > MAX_POOL_MUTATION_CANDIDATES {
            return Err(component_limit(false));
        }
        let mut update_rank = |ancestor: &Byte32, reduction: Aggregate| -> Result<(), Error> {
            let parent = entries.get(ancestor).ok_or(Error::Stale)?;
            let total = descendants.get_mut(ancestor).ok_or(Error::Stale)?;
            ranks.remove(&EvictionRank::new(parent, *total, snapshot)?);
            *total = total.sub(reduction)?;
            ranks.insert(EvictionRank::new(parent, *total, snapshot)?);
            Ok(())
        };
        let mut reductions: BTreeMap<Byte32, Aggregate> = BTreeMap::new();
        for hash in &closure {
            let entry = entries.get(hash).ok_or(Error::Stale)?;
            let own = Aggregate::one(accepted(entry)?);
            for ancestor in ancestor_hashes(entries, hash, config.max_ancestors_count)? {
                if closure.contains(&ancestor) {
                    continue;
                }
                if closure.len() == 1 {
                    update_rank(&ancestor, own)?;
                } else {
                    let reduction = reductions.entry(ancestor).or_default();
                    *reduction = reduction.add(own)?;
                }
            }
        }
        // No selection occurs within a closure: settle every surviving rank
        // once before the next round chooses its root.
        for (ancestor, reduction) in reductions {
            update_rank(&ancestor, reduction)?;
        }
        for hash in closure {
            let entry = entries.remove(&hash).ok_or(Error::Stale)?;
            ranks.remove(&EvictionRank::new(
                &entry,
                descendants.remove(&hash).ok_or(Error::Stale)?,
                snapshot,
            )?);
            charge = charge
                .checked_sub(owner_amount(&entry)?)
                .ok_or_else(overflow)?;
            removed.insert(hash.clone());
            causes.insert(hash, Removal::Capacity);
        }
    }
    Ok(())
}

/// Descendant-before-parent effects have the same deterministic hash tie order
/// as the mutation closure; an ordinary removal cannot leave a live descendant.
pub(super) fn removal_order(
    members: &Members,
    removed: &BTreeSet<Byte32>,
) -> Result<Vec<Byte32>, Error> {
    let mut counts: BTreeMap<_, usize> = removed.iter().map(|hash| (hash.clone(), 0)).collect();
    for hash in removed {
        for parent in &accepted(members.get(hash).ok_or(Error::Stale)?)?.parents {
            if let Some(count) = counts.get_mut(parent) {
                *count = count.checked_add(1).ok_or_else(overflow)?;
            }
        }
    }
    let mut leaves: BTreeSet<_> = counts
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(hash, _)| hash.clone())
        .collect();
    let mut order = Vec::new();
    while let Some(hash) = leaves.pop_first() {
        for parent in &accepted(members.get(&hash).ok_or(Error::Stale)?)?.parents {
            if let Some(count) = counts.get_mut(parent) {
                *count = count.checked_sub(1).ok_or_else(overflow)?;
                if *count == 0 {
                    leaves.insert(parent.clone());
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
fn history(
    old: &Entry,
    candidate_inputs: &BTreeSet<OutPoint>,
    removed: &BTreeSet<Byte32>,
    graph: &mut Graph<'_>,
) -> Result<Option<Arc<Entry>>, Error> {
    let accepted = accepted(old)?;
    let mut triggers = BTreeSet::new();
    for cell in accepted
        .transaction
        .resolved_inputs
        .iter()
        .chain(&accepted.transaction.resolved_cell_deps)
        .chain(&accepted.transaction.resolved_dep_groups)
    {
        let spender = graph.spender(&cell.out_point)?;
        if candidate_inputs.contains(&cell.out_point)
            || spender.as_ref().is_some_and(|hash| !removed.contains(hash))
            || (cell.transaction_info.is_none() && removed.contains(&cell.out_point.tx_hash()))
        {
            triggers.insert(DependencyKey::Cell(compact_packed(&cell.out_point)));
        }
    }
    let require_all = !triggers.is_empty();
    if !require_all {
        triggers.extend(
            old.transaction
                .input_pts_iter()
                .map(|point| DependencyKey::Cell(compact_packed(&point))),
        );
    }
    if triggers.is_empty() {
        return Ok(None);
    }
    Ok(Some(Arc::new(Entry {
        transaction: Arc::clone(&old.transaction),
        arrival: old.arrival,
        source: Source::Recovery,
        phase: Phase::Replaced {
            triggers,
            require_all,
        },
    })))
}

pub(super) fn removal(
    store: &Store,
    root: &Arc<Entry>,
    config: &TxPoolConfig,
    reason: Option<Reject>,
) -> Result<Plan, Error> {
    let (view, _) = store.snapshot();
    let mut plan = Plan::new(view, super::notice::Class::Trusted, Default::default());
    plan.observe_owner(&root.hash(), Some(root))?;
    if root.accepted().is_none() {
        plan.edit(Some(Arc::clone(root)), None, None)?;
    } else {
        let mut graph = Graph::new(store, &mut plan);
        let removed = graph.descendants(
            [root.hash()],
            &BTreeSet::new(),
            store.budget.limits.accepted.items,
        )?;
        // Local administration promises complete descendant removal. Its
        // atomic work is bounded by accepted residency, as with reconciliation.
        // Expiry emits per-entry notices and may advance by leaf-first pages.
        let limit = if reason.is_none() {
            removed.len()
        } else {
            MAX_POOL_MUTATION_CANDIDATES
        };
        let mut order = removal_order(graph.entries(), &removed)?;
        order.truncate(limit);
        let removed_totals = if reason.is_some() && order.len() > 1 {
            Some(graph.removal_totals(&order, config.max_ancestors_count)?)
        } else {
            None
        };
        for hash in order {
            let old = graph.require(&hash)?;
            let effect = if let Some(reason) = &reason {
                let snapshot = if let Some(totals) = &removed_totals {
                    let (ancestors, descendants) = totals.get(&hash).ok_or(Error::Stale)?;
                    self::snapshot(&old, *ancestors, *descendants)?
                } else {
                    graph.entry_snapshot(&hash, config.max_ancestors_count)?
                };
                Some(Effect::rejected(
                    &hash,
                    reason.clone(),
                    Some(snapshot),
                    true,
                )?)
            } else {
                None
            };
            graph.plan.edit(Some(old), None, effect)?;
        }
    }
    if reason.is_none() {
        plan.notify(Effect::reset());
    }
    Ok(plan)
}

#[cfg(test)]
#[path = "tests/membership_trim.rs"]
mod trim_tests;
