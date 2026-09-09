//! Reliable chain and clear commands reconcile a bounded captured population.
//! Canonical block facts are read before Apply; the lifecycle writer publishes
//! one paired snapshot and owner change, followed by ordered notices.
use super::{
    jobs::environment,
    membership::{self, Members},
    model::{DependencyKey, Entry, Error, Phase, Source, status},
    notice::{Class, Effect},
    store::{Plan, Store},
};
use crate::{
    callback::CallbackEvent,
    error::Reject,
    service::{BoundedTransaction, ChainReorgArgs, TxVerificationResult},
    util::compact_packed,
};
use ckb_app_config::TxPoolConfig;
use ckb_snapshot::Snapshot;
use ckb_types::core::error::OutPointError;
use ckb_verification::cache::ScriptVerificationRules;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[cfg(test)]
#[path = "tests/chain.rs"]
mod tests;

fn current_source(entry: &Entry, previous: &Snapshot, snapshot: &Snapshot) -> Option<Source> {
    let proposed = status(snapshot, &entry.proposal()) != super::model::Status::Pending;
    let expired = !proposed && status(previous, &entry.proposal()) != super::model::Status::Pending;
    Some(match entry.source {
        Source::Remote { peer, deadline, .. } if proposed => Source::Proposal {
            remote: Some((peer, deadline)),
        },
        Source::Proposal {
            remote: Some((peer, deadline)),
        } if expired => Source::Remote {
            peer,
            deadline,
            cycles: None,
        },
        Source::Proposal { remote: None } if expired => return None,
        source => source,
    })
}

/// Detailed reconciliation can exceed the recovery account even when the old
/// accepted pool fit. Keep a dependency-ordered bounded recovery population,
/// preserving preaccepted peer origins and bans, and invalidate the old view once.
pub(super) fn recover_bounded(store: &Store, command: &ChainReorgArgs) -> Result<Plan, Error> {
    #[cfg(feature = "profiling")]
    let _span =
        tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.chain.recover").entered();
    let (detached_blocks, attached_blocks, snapshot) = match command {
        ChainReorgArgs::Detailed {
            detached_blocks,
            attached_blocks,
            snapshot,
        } => (detached_blocks, attached_blocks, snapshot),
        ChainReorgArgs::ReplaceGeneration { snapshot } => {
            return clear(store, Some(Arc::clone(snapshot)), false);
        }
    };
    let (view, old_snapshot, owners, reads) = store.capture(false);
    let mut attached = BTreeSet::new();
    for block in attached_blocks {
        attached.extend(
            block
                .transactions()
                .iter()
                .map(|transaction| transaction.hash()),
        );
    }
    let mut candidates = BTreeMap::new();
    for entry in &owners {
        if attached.contains(&entry.hash()) {
            continue;
        }
        let source = if entry.accepted().is_some() {
            Source::Recovery
        } else if let Some(source) = current_source(entry, &old_snapshot, snapshot) {
            source
        } else {
            continue;
        };
        candidates.insert(
            entry.hash(),
            Arc::new(Entry {
                transaction: Arc::clone(&entry.transaction),
                arrival: entry.arrival,
                source,
                phase: Phase::Resolve,
            }),
        );
    }
    for block in detached_blocks {
        for transaction in block.transactions().iter().skip(1) {
            if attached.contains(&transaction.hash()) {
                continue;
            }
            // Detached transactions are optional pool candidates, just as in
            // detailed reconciliation. Pool bounds cannot reject chain progress.
            let Ok(transaction) = BoundedTransaction::try_new(transaction.clone()) else {
                continue;
            };
            let transaction = transaction.into_transaction();
            candidates.insert(
                transaction.hash(),
                Arc::new(Entry {
                    transaction,
                    arrival: store.next_arrival()?,
                    source: Source::Recovery,
                    phase: Phase::Resolve,
                }),
            );
        }
    }
    let mut candidates: Vec<_> = candidates.into_values().collect();
    candidates.sort_by_key(|entry| (entry.source.priority(), entry.arrival, entry.hash()));
    crate::dependency_sort::sort_by_dependencies(&mut candidates, |entry| {
        entry.transaction.as_ref()
    })
    .map_err(|_| Error::Full("recovery dependency ordering".into()))?;
    store.budget.limits.retain_fitting(&mut candidates)?;
    let mut retained = BTreeMap::new();
    for entry in candidates {
        // Queue order follows the bounded parent-before-child recovery order.
        let arrival = if matches!(entry.source, Source::Recovery) {
            store.next_arrival()?
        } else {
            entry.arrival
        };
        retained.insert(
            entry.hash(),
            Arc::new(Entry {
                arrival,
                ..entry.as_ref().clone()
            }),
        );
    }
    let mut plan = Plan::new(view, Class::Critical);
    plan.reads = reads;
    plan.snapshot = Some(Arc::clone(snapshot));
    plan.invalidate_view = true;
    for entry in owners {
        let after = retained.remove(&entry.hash());
        plan.edit(Some(entry), after)?;
    }
    for entry in retained.into_values() {
        plan.edit(None, Some(entry))?;
    }
    plan.effects.push(Effect::reset());
    if !attached_blocks.is_empty() {
        plan.effects.push(Effect {
            blocks: attached_blocks
                .iter()
                .map(|block| Arc::new(block.clone()))
                .collect(),
            ..Effect::default()
        });
    }
    for block in attached_blocks {
        for transaction in block.transactions().iter().skip(1) {
            plan.committed.push((
                compact_packed(&transaction.proposal_short_id()),
                compact_packed(&transaction.hash()),
            ));
        }
    }
    Ok(plan)
}

pub(super) fn clear(
    store: &Store,
    snapshot: Option<Arc<Snapshot>>,
    pipeline_only: bool,
) -> Result<Plan, Error> {
    let (view, current, owners, reads) = store.capture(false);
    let mut plan = Plan::new(view, Class::Critical);
    plan.reads = reads;
    plan.invalidate_view = true;
    plan.snapshot = Some(snapshot.unwrap_or(current));
    plan.clear_all = !pipeline_only;
    for entry in owners {
        if !pipeline_only || entry.accepted().is_none() {
            plan.edit(Some(entry), None)?;
        }
    }
    plan.effects.push(Effect::reset());
    Ok(plan)
}

/// Invoke while holding the sole chain preparation pause, so continuous
/// ordinary ingress cannot starve an authoritative view change by OCC retries.
pub(super) fn reconcile(
    store: &Store,
    command: &ChainReorgArgs,
    config: &TxPoolConfig,
) -> Result<Plan, Error> {
    #[cfg(feature = "profiling")]
    let _span =
        tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.chain.reconcile").entered();
    let (detached_blocks, attached_blocks, snapshot) = match command {
        ChainReorgArgs::Detailed {
            detached_blocks,
            attached_blocks,
            snapshot,
        } => (detached_blocks, attached_blocks, snapshot),
        ChainReorgArgs::ReplaceGeneration { snapshot } => {
            return clear(store, Some(Arc::clone(snapshot)), false);
        }
    };
    if attached_blocks
        .back()
        .is_some_and(|block| block.hash() != snapshot.tip_hash())
    {
        return Err(Error::Fault("chain snapshot tip"));
    }
    let (view, old_snapshot, owners, reads) = store.capture(false);
    let old: BTreeMap<_, _> = owners
        .into_iter()
        .map(|entry| (entry.hash(), entry))
        .collect();
    let accepted: Members = old
        .iter()
        .filter(|(_, entry)| entry.accepted().is_some())
        .map(|(hash, entry)| (hash.clone(), Arc::clone(entry)))
        .collect();
    let mut attached = BTreeSet::new();
    let mut spent = BTreeSet::new();
    for block in attached_blocks {
        for (index, tx) in block.transactions().iter().enumerate() {
            attached.insert(compact_packed(&tx.hash()));
            if index != 0 {
                spent.extend(tx.input_pts_iter().map(|point| compact_packed(&point)));
            }
        }
    }
    let detached_headers: BTreeSet<_> = detached_blocks
        .iter()
        .map(|block| compact_packed(&block.hash()))
        .collect();
    let mut detached = Vec::new();
    let mut seen = BTreeSet::new();
    for block in detached_blocks {
        for tx in block.transactions().iter().skip(1) {
            if attached.contains(&tx.hash()) || !seen.insert(tx.hash()) {
                continue;
            }
            if let Ok(tx) = BoundedTransaction::try_new(tx.clone()) {
                detached.push(tx.into_transaction());
            }
        }
    }
    crate::dependency_sort::sort_by_dependencies(&mut detached, |tx| tx.as_ref())
        .map_err(|_| Error::Full("detached recovery ordering".into()))?;
    let child_index = membership::children(&accepted);
    let mut conflicts = BTreeMap::new();
    let mut recovery = BTreeSet::new();
    for (hash, entry) in &accepted {
        if attached.contains(hash) {
            continue;
        }
        let value = entry.accepted().ok_or(Error::Stale)?;
        let conflict = entry
            .transaction
            .input_pts_iter()
            .chain(value.dependencies())
            .find(|point| spent.contains(point));
        if let Some(point) = conflict {
            conflicts.insert(hash.clone(), compact_packed(&point));
            continue;
        }
        let detached_location = value
            .transaction
            .resolved_inputs
            .iter()
            .chain(&value.transaction.resolved_cell_deps)
            .chain(&value.transaction.resolved_dep_groups)
            .any(|cell| {
                cell.transaction_info
                    .as_ref()
                    .is_some_and(|info| detached_headers.contains(&info.block_hash))
            });
        let detached_header = entry
            .transaction
            .header_deps_iter()
            .any(|hash| detached_headers.contains(&hash));
        let old_env = environment(value.status(&old_snapshot), &old_snapshot);
        let new_env = environment(value.status(snapshot), snapshot);
        let rules_changed = ScriptVerificationRules::from_env(old_snapshot.consensus(), &old_env)
            != ScriptVerificationRules::from_env(snapshot.consensus(), &new_env);
        if detached_location
            || detached_header
            || rules_changed
            || (!detached_blocks.is_empty() && value.context_sensitive)
        {
            recovery.insert(hash.clone());
        }
    }
    // Waiting owners retain only missing trigger keys, so check their full
    // declared footprint as well. A committed spend is a terminal chain fact,
    // even if an unrelated missing parent has never produced a wake.
    for (hash, entry) in &old {
        if !entry.preaccepted() || attached.contains(hash) {
            continue;
        }
        if let Some(point) = entry
            .declared_dependencies()
            .into_iter()
            .chain(entry.dependencies())
            .filter_map(|key| match key {
                DependencyKey::Cell(point) => Some(point),
                _ => None,
            })
            .find(|point| spent.contains(point))
        {
            conflicts.insert(hash.clone(), compact_packed(&point));
        }
    }
    // Descendant closure stops at a now-committed producer: its output has a
    // canonical backing on the new chain and surviving children may retain it.
    let mut stack: Vec<_> = conflicts
        .iter()
        .map(|(hash, point)| (hash.clone(), point.clone()))
        .collect();
    while let Some((hash, point)) = stack.pop() {
        if let Some(children) = child_index.get(&hash) {
            for child in children {
                if attached.contains(child) || conflicts.contains_key(child) {
                    continue;
                }
                conflicts.insert(child.clone(), point.clone());
                stack.push((child.clone(), point.clone()));
            }
        }
    }
    let mut stack: Vec<_> = recovery.iter().cloned().collect();
    while let Some(hash) = stack.pop() {
        if let Some(children) = child_index.get(&hash) {
            for child in children {
                if !attached.contains(child)
                    && !conflicts.contains_key(child)
                    && recovery.insert(child.clone())
                {
                    stack.push(child.clone());
                }
            }
        }
    }
    let mut old_totals = None;
    let mut after = BTreeMap::new();
    for (hash, entry) in &old {
        if attached.contains(hash) || conflicts.contains_key(hash) {
            continue;
        }
        if let Some(value) = entry.accepted() {
            if recovery.contains(hash) {
                after.insert(
                    hash.clone(),
                    Arc::new(Entry {
                        transaction: Arc::clone(&entry.transaction),
                        arrival: entry.arrival,
                        source: Source::Recovery,
                        phase: Phase::Resolve,
                    }),
                );
            } else {
                let mut value = value.clone();
                value.parents.retain(|parent| !attached.contains(parent));
                let unchanged = entry
                    .accepted()
                    .is_some_and(|old| old.parents == value.parents);
                after.insert(
                    hash.clone(),
                    if unchanged {
                        Arc::clone(entry)
                    } else {
                        entry.with_phase(Phase::Accepted(value))
                    },
                );
            }
            continue;
        }
        let Some(source) = current_source(entry, &old_snapshot, snapshot) else {
            continue;
        };
        after.insert(
            hash.clone(),
            if source != entry.source || matches!(entry.phase, Phase::Verify(_)) {
                Arc::new(Entry {
                    transaction: Arc::clone(&entry.transaction),
                    arrival: entry.arrival,
                    source,
                    phase: Phase::Resolve,
                })
            } else {
                Arc::clone(entry)
            },
        );
    }
    // The detached block's witness wins for its own raw hash. Existing peer
    // dependents above retain their source and deadline through re-resolution.
    for transaction in detached {
        let hash = compact_packed(&transaction.hash());
        let arrival = match old.get(&hash) {
            Some(old) => old.arrival,
            None => store.next_arrival()?,
        };
        after.insert(
            hash,
            Arc::new(Entry {
                transaction,
                arrival,
                source: Source::Recovery,
                phase: Phase::Resolve,
            }),
        );
    }
    let mut plan = Plan::new(view, Class::Critical);
    plan.reads = reads;
    plan.snapshot = Some(Arc::clone(snapshot));
    for (hash, old) in &old {
        let next = after.get(hash);
        if !next.is_some_and(|next| Arc::ptr_eq(next, old)) {
            plan.edit(Some(Arc::clone(old)), next.cloned())?;
        }
        if attached.contains(hash) {
            if old.preaccepted()
                && let Some(peer) = old.source.residency_peer()
            {
                plan.effects.push(Effect {
                    relay: Some(TxVerificationResult::Ok {
                        original_peer: Some(peer),
                        tx_hash: hash.clone(),
                    }),
                    ..Effect::default()
                });
            }
        } else if let Some(point) = conflicts.get(hash) {
            let callback = if old.accepted().is_some() {
                if old_totals.is_none() {
                    old_totals = Some(membership::aggregates(
                        &accepted,
                        config.max_ancestors_count,
                    )?);
                }
                let (ancestors, descendants) = old_totals
                    .as_ref()
                    .and_then(|totals| totals.get(hash))
                    .ok_or(Error::Stale)?;
                Some(membership::snapshot(old, *ancestors, *descendants)?)
            } else {
                None
            };
            plan.effects.push(Effect::rejected(
                hash,
                Reject::Resolve(OutPointError::Dead(point.clone())),
                callback,
                old.accepted().is_some() || old.source.residency_peer().is_some(),
            )?);
        } else if !after.contains_key(hash) && old.source.residency_peer().is_some() {
            plan.effects.push(Effect {
                relay: Some(TxVerificationResult::Reject {
                    tx_hash: hash.clone(),
                }),
                ..Effect::default()
            });
        }
    }
    for (hash, entry) in &after {
        if !old.contains_key(hash) {
            plan.edit(None, Some(Arc::clone(entry)))?;
        }
    }
    let final_accepted: Members = after
        .into_iter()
        .filter(|(_, entry)| entry.accepted().is_some())
        .collect();
    let mut final_totals = None;
    for (hash, entry) in &final_accepted {
        if old
            .get(hash)
            .and_then(|entry| entry.accepted())
            .is_some_and(|old| {
                old.status(&old_snapshot)
                    != entry
                        .accepted()
                        .map(|value| value.status(snapshot))
                        .unwrap_or(super::model::Status::Pending)
            })
        {
            if final_totals.is_none() {
                final_totals = Some(membership::aggregates(
                    &final_accepted,
                    config.max_ancestors_count,
                )?);
            }
            let (ancestors, descendants) = final_totals
                .as_ref()
                .and_then(|totals| totals.get(hash))
                .ok_or(Error::Stale)?;
            let value = membership::snapshot(entry, *ancestors, *descendants)?;
            let callback = if entry.accepted().ok_or(Error::Stale)?.status(snapshot)
                == super::model::Status::Proposed
            {
                CallbackEvent::Proposed(value)
            } else {
                CallbackEvent::Pending(value)
            };
            plan.effects.push(Effect {
                callback: Some(callback),
                ..Effect::default()
            });
        }
    }
    for block in attached_blocks.iter().chain(detached_blocks) {
        for tx in block.transactions() {
            for point in tx.output_pts_iter().chain(tx.input_pts_iter()) {
                plan.wake
                    .insert(DependencyKey::Cell(compact_packed(&point)));
            }
        }
    }
    if !attached_blocks.is_empty() {
        plan.effects.push(Effect {
            blocks: attached_blocks
                .iter()
                .map(|block| Arc::new(block.clone()))
                .collect(),
            ..Effect::default()
        });
    }
    for block in attached_blocks {
        for transaction in block.transactions().iter().skip(1) {
            plan.committed.push((
                compact_packed(&transaction.proposal_short_id()),
                compact_packed(&transaction.hash()),
            ));
        }
    }
    Ok(plan)
}
