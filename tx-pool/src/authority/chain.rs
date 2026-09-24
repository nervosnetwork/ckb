//! Reliable chain and clear commands reconcile a bounded captured population.
//! Canonical block facts are read before Apply; the lifecycle writer publishes
//! one paired snapshot and owner change, followed by ordered notices.
use super::{
    jobs::environment,
    membership::{self, Members},
    model::{DependencyKey, Entry, Error, Phase, Source, status},
    notice::{Class, Effect},
    store::{Captured, Plan, Store},
};
use crate::{
    error::Reject,
    service::{BoundedTransaction, ChainReorgArgs, TxVerificationResult},
    util::compact_packed,
};
use ckb_app_config::TxPoolConfig;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{BlockView, TransactionView, error::OutPointError},
    packed::{Byte32, OutPoint},
};
use ckb_verification::cache::ScriptVerificationRules;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

#[cfg(test)]
#[path = "tests/chain.rs"]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ClearScope {
    All,
    Unaccepted,
}

fn current_source(entry: &Entry, previous: &Snapshot, snapshot: &Snapshot) -> Option<Source> {
    let proposed = status(snapshot, &entry.proposal()) != super::model::Status::Pending;
    let expired = !proposed && status(previous, &entry.proposal()) != super::model::Status::Pending;
    if proposed {
        Some(entry.source.promote_remote())
    } else if expired {
        entry.source.after_proposal_window()
    } else {
        Some(entry.source)
    }
}

/// Detailed reconciliation can exceed the recovery account even when the old
/// accepted pool fit. Keep a dependency-ordered bounded recovery population,
/// preserving preaccepted peer origins and bans, and invalidate the old view once.
pub(super) fn recover_bounded(store: &Store, command: &ChainReorgArgs) -> Result<Plan, Error> {
    #[cfg(feature = "profiling")]
    let _span =
        tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.chain.recover").entered();
    let snapshot = command.snapshot();
    let Some(fork) = command.fork() else {
        return clear(store, Some(Arc::clone(snapshot)), ClearScope::All);
    };
    let detached_blocks = &fork.detached_blocks;
    let attached_blocks = &fork.attached_blocks;
    let Captured {
        view,
        snapshot: old_snapshot,
        owners,
        reads,
    } = store.capture_all();
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
    let mut plan = Plan::new(view, Class::Critical, reads);
    for entry in owners {
        let after = retained.remove(&entry.hash());
        plan.edit(Some(entry), after, None)?;
    }
    for entry in retained.into_values() {
        plan.edit(None, Some(entry), None)?;
    }
    plan.reset(Arc::clone(snapshot));
    plan.chain(
        Arc::clone(snapshot),
        attached_blocks.iter().map(|block| Arc::new(block.clone())),
    );
    Ok(plan)
}

pub(super) fn clear(
    store: &Store,
    snapshot: Option<Arc<Snapshot>>,
    scope: ClearScope,
) -> Result<Plan, Error> {
    let Captured {
        view,
        snapshot: current,
        owners,
        reads,
    } = store.capture_all();
    let mut plan = Plan::new(view, Class::Critical, reads);
    for entry in owners {
        if scope == ClearScope::All || entry.accepted().is_none() {
            plan.edit(Some(entry), None, None)?;
        }
    }
    let snapshot = snapshot.unwrap_or(current);
    match scope {
        ClearScope::All => plan.replace_generation(snapshot),
        ClearScope::Unaccepted => plan.reset(snapshot),
    }
    Ok(plan)
}

/// Canonical facts from this fork transition. The detached bodies are already
/// bounded and ordered for recovery before owner policy examines them.
struct ChainFacts {
    attached: BTreeSet<Byte32>,
    spent: BTreeSet<OutPoint>,
    detached_headers: BTreeSet<Byte32>,
    detached_transactions: BTreeSet<Byte32>,
    detached: Vec<Arc<TransactionView>>,
}

impl ChainFacts {
    fn from_blocks(
        detached_blocks: &VecDeque<BlockView>,
        attached_blocks: &VecDeque<BlockView>,
    ) -> Result<Self, Error> {
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
        let detached_headers = detached_blocks
            .iter()
            .map(|block| compact_packed(&block.hash()))
            .collect();
        let mut detached = Vec::new();
        let mut detached_transactions = BTreeSet::new();
        for block in detached_blocks {
            for (index, tx) in block.transactions().iter().enumerate() {
                if attached.contains(&tx.hash()) || !detached_transactions.insert(tx.hash()) {
                    continue;
                }
                if index != 0
                    && let Ok(tx) = BoundedTransaction::try_new(tx.clone())
                {
                    detached.push(tx.into_transaction());
                }
            }
        }
        crate::dependency_sort::sort_by_dependencies(&mut detached, |tx| tx.as_ref())
            .map_err(|_| Error::Full("detached recovery ordering".into()))?;
        Ok(Self {
            attached,
            spent,
            detached_headers,
            detached_transactions,
            detached,
        })
    }
}

struct Affected {
    conflicts: BTreeMap<Byte32, OutPoint>,
    recovery: BTreeSet<Byte32>,
}

/// Classify every existing owner, then close accepted conflicts and recovery
/// over descendants without crossing a newly committed producer.
fn affected_owners(
    old: &BTreeMap<Byte32, Arc<Entry>>,
    accepted: &Members,
    facts: &ChainFacts,
    detached_any: bool,
    previous: &Snapshot,
    snapshot: &Snapshot,
) -> Result<Affected, Error> {
    let child_index = membership::children(accepted);
    let mut conflicts = BTreeMap::new();
    let mut recovery = BTreeSet::new();
    for (hash, entry) in accepted {
        if facts.attached.contains(hash) {
            continue;
        }
        let value = entry.accepted().ok_or(Error::Stale)?;
        let conflict = entry
            .transaction
            .input_pts_iter()
            .chain(value.dependencies())
            .find(|point| facts.spent.contains(point));
        if let Some(point) = conflict {
            conflicts.insert(hash.clone(), compact_packed(&point));
            continue;
        }
        // A pool-resolved cell retains its outpoint identity after its
        // producer commits, including dep-group members.
        let detached_producer = value
            .transaction
            .resolved_inputs
            .iter()
            .chain(&value.transaction.resolved_cell_deps)
            .chain(&value.transaction.resolved_dep_groups)
            .any(|cell| {
                facts
                    .detached_transactions
                    .contains(&cell.out_point.tx_hash())
            });
        let detached_header = entry
            .transaction
            .header_deps_iter()
            .any(|hash| facts.detached_headers.contains(&hash));
        let old_env = environment(value.status(previous), previous);
        let new_env = environment(value.status(snapshot), snapshot);
        let rules_changed = ScriptVerificationRules::from_env(previous.consensus(), &old_env)
            != ScriptVerificationRules::from_env(snapshot.consensus(), &new_env);
        if detached_producer
            || detached_header
            || rules_changed
            || (detached_any && value.context_sensitive)
        {
            recovery.insert(hash.clone());
        }
    }
    // Waiting owners retain only missing triggers; their full declared reads
    // must still reject a now-committed spend.
    for (hash, entry) in old {
        if !entry.preaccepted() || facts.attached.contains(hash) {
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
            .find(|point| facts.spent.contains(point))
        {
            conflicts.insert(hash.clone(), compact_packed(&point));
        }
    }
    let mut stack: Vec<_> = conflicts
        .iter()
        .map(|(hash, point)| (hash.clone(), point.clone()))
        .collect();
    while let Some((hash, point)) = stack.pop() {
        if let Some(children) = child_index.get(&hash) {
            for child in children {
                if facts.attached.contains(child) || conflicts.contains_key(child) {
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
                if !facts.attached.contains(child)
                    && !conflicts.contains_key(child)
                    && recovery.insert(child.clone())
                {
                    stack.push(child.clone());
                }
            }
        }
    }
    Ok(Affected {
        conflicts,
        recovery,
    })
}

/// Produce the next owner population; only detached transactions acquire a new
/// arrival, and their block witness wins over an existing body of the same hash.
fn successor_population(
    store: &Store,
    old: &BTreeMap<Byte32, Arc<Entry>>,
    facts: &mut ChainFacts,
    affected: &Affected,
    previous: &Snapshot,
    snapshot: &Snapshot,
) -> Result<BTreeMap<Byte32, Arc<Entry>>, Error> {
    let mut after = BTreeMap::new();
    for (hash, entry) in old {
        if facts.attached.contains(hash) || affected.conflicts.contains_key(hash) {
            continue;
        }
        if let Some(value) = entry.accepted() {
            if affected.recovery.contains(hash) {
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
                let next = if value
                    .parents
                    .iter()
                    .any(|parent| facts.attached.contains(parent))
                {
                    let mut value = value.clone();
                    value
                        .parents
                        .retain(|parent| !facts.attached.contains(parent));
                    entry.with_phase(Phase::Accepted(value))
                } else {
                    Arc::clone(entry)
                };
                after.insert(hash.clone(), next);
            }
            continue;
        }
        let Some(source) = current_source(entry, previous, snapshot) else {
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
    for transaction in facts.detached.drain(..) {
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
    Ok(after)
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
    let snapshot = command.snapshot();
    let Some(fork) = command.fork() else {
        return clear(store, Some(Arc::clone(snapshot)), ClearScope::All);
    };
    let detached_blocks = &fork.detached_blocks;
    let attached_blocks = &fork.attached_blocks;
    if attached_blocks
        .back()
        .is_some_and(|block| block.hash() != snapshot.tip_hash())
    {
        return Err(Error::Fault("chain snapshot tip"));
    }
    let Captured {
        view,
        snapshot: old_snapshot,
        owners,
        reads,
    } = store.capture_all();
    let old: BTreeMap<_, _> = owners
        .into_iter()
        .map(|entry| (entry.hash(), entry))
        .collect();
    let accepted: Members = old
        .iter()
        .filter(|(_, entry)| entry.accepted().is_some())
        .map(|(hash, entry)| (hash.clone(), Arc::clone(entry)))
        .collect();
    let mut facts = ChainFacts::from_blocks(detached_blocks, attached_blocks)?;
    let affected = affected_owners(
        &old,
        &accepted,
        &facts,
        !detached_blocks.is_empty(),
        &old_snapshot,
        snapshot,
    )?;
    let after = successor_population(store, &old, &mut facts, &affected, &old_snapshot, snapshot)?;
    let mut old_totals = None;
    let mut plan = Plan::new(view, Class::Critical, reads);
    for (hash, old) in &old {
        let next = after.get(hash);
        let effect = if facts.attached.contains(hash) {
            old.preaccepted_peer().map(|peer| {
                Effect::relay(TxVerificationResult::Ok {
                    original_peer: Some(peer),
                    tx_hash: hash.clone(),
                })
            })
        } else if let Some(point) = affected.conflicts.get(hash) {
            let callback = if old.accepted().is_some() {
                if old_totals.is_none() {
                    old_totals = Some(membership::aggregates(
                        &accepted,
                        config.max_ancestors_count,
                    )?);
                }
                let totals = old_totals
                    .as_ref()
                    .and_then(|totals| totals.get(hash))
                    .ok_or(Error::Stale)?;
                Some(membership::entry_snapshot(old, *totals)?)
            } else {
                None
            };
            Some(Effect::removed(
                old,
                Reject::Resolve(OutPointError::Dead(point.clone())),
                callback,
            )?)
        } else {
            None
        };
        if !next.is_some_and(|next| Arc::ptr_eq(next, old)) {
            plan.edit(Some(Arc::clone(old)), next.cloned(), effect)?;
        } else if let Some(effect) = effect {
            plan.notify(effect);
        }
    }
    for (hash, entry) in &after {
        if !old.contains_key(hash) {
            plan.edit(None, Some(Arc::clone(entry)), None)?;
        }
    }
    let final_accepted: Members = after
        .into_iter()
        .filter(|(_, entry)| entry.accepted().is_some())
        .collect();
    let mut final_totals = None;
    for (hash, entry) in &final_accepted {
        let Some(previous) = old.get(hash).and_then(|entry| entry.accepted()) else {
            continue;
        };
        let status = entry.accepted().ok_or(Error::Stale)?.status(snapshot);
        if previous.status(&old_snapshot) == status {
            continue;
        }
        if final_totals.is_none() {
            final_totals = Some(membership::aggregates(
                &final_accepted,
                config.max_ancestors_count,
            )?);
        }
        let totals = final_totals
            .as_ref()
            .and_then(|totals| totals.get(hash))
            .ok_or(Error::Stale)?;
        let value = membership::entry_snapshot(entry, *totals)?;
        plan.notify(Effect::projected(value, status));
    }
    for block in attached_blocks.iter().chain(detached_blocks) {
        for tx in block.transactions() {
            for point in tx.output_pts_iter().chain(tx.input_pts_iter()) {
                plan.signal_available(DependencyKey::Cell(point));
            }
        }
    }
    plan.chain(
        Arc::clone(snapshot),
        attached_blocks.iter().map(|block| Arc::new(block.clone())),
    );
    Ok(plan)
}
