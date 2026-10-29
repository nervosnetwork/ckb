//! Reliable chain and clear commands reconcile a bounded captured population.
//! Canonical block facts are read before Apply; the lifecycle writer publishes
//! one paired snapshot and owner change, followed by ordered notices.
use super::{
    jobs::environment,
    membership::{self, Members},
    model::{DependencyKey, Entry, Error, Phase, Source, Status},
    notice::{Class, Effect},
    store::{Captured, Plan, Store},
};
use crate::{
    error::Reject,
    service::{BoundedTransaction, ChainReorgArgs, TxVerificationResult},
    util::compact_packed,
};
use ckb_app_config::TxPoolConfig;
use ckb_proposal_table::ProposalView;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{BlockView, TransactionView, error::OutPointError},
    packed::{Byte32, OutPoint},
    prelude::{Entity, Reader},
};
use ckb_verification::cache::ScriptVerificationRules;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
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
    let id = entry.transaction.proposal_short_id_reader();
    let proposed = snapshot.proposals().status_reader(id) != Status::Pending;
    let expired = !proposed && previous.proposals().status_reader(id) != Status::Pending;
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
        ..
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
    for mut entry in candidates {
        // These owners are newly constructed above. Assign recovery order before
        // publishing them; remote origins retain their original arrival.
        if matches!(entry.source, Source::Recovery) {
            Arc::make_mut(&mut entry).arrival = store.next_arrival()?;
        }
        retained.insert(entry.hash(), entry);
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
        ..
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
/// Membership sets are lookup-only; their keys share the retained block backing
/// during preparation and do not survive in the resulting Plan.
struct ChainFacts {
    attached: HashSet<Byte32>,
    spent: HashSet<OutPoint>,
    detached_headers: HashSet<Byte32>,
    detached_transactions: HashSet<Byte32>,
    detached: Vec<Arc<TransactionView>>,
}

impl ChainFacts {
    fn from_blocks(
        detached_blocks: &VecDeque<BlockView>,
        attached_blocks: &VecDeque<BlockView>,
    ) -> Result<Self, Error> {
        let mut attached = HashSet::new();
        let mut spent = HashSet::new();
        for block in attached_blocks {
            for (index, tx) in block.transactions().iter().enumerate() {
                attached.insert(tx.hash());
                if index != 0 {
                    spent.extend(tx.input_pts_iter());
                }
            }
        }
        let detached_headers = detached_blocks.iter().map(|block| block.hash()).collect();
        let mut detached = Vec::new();
        let mut detached_transactions = HashSet::new();
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
    store: &Store,
    plan: &mut Plan,
    old: &BTreeMap<Byte32, Arc<Entry>>,
    facts: &ChainFacts,
    previous: &Snapshot,
    snapshot: &Snapshot,
) -> Result<Affected, Error> {
    // Accepted input/dep readers are already indexed. A small spent set can
    // narrow their checks without scanning every accepted dependency; retain
    // these original relation reads alongside the captured owner revisions.
    // Larger block spend sets keep the owner-bounded direct lookup below.
    let readers = if !facts.spent.is_empty() && facts.spent.len() <= old.len() {
        let mut readers = HashSet::new();
        for point in &facts.spent {
            for hash in plan.readers(store, point.clone())? {
                if !facts.attached.contains(&hash) {
                    readers.insert(hash);
                }
            }
        }
        Some(readers)
    } else {
        None
    };
    // Canonical VM activation uses the next block's epoch for every inflight
    // phase. Derive that view-wide rule once, independently of proposal phase.
    let rules = |snapshot: &Snapshot| {
        ScriptVerificationRules::from_env(
            snapshot.consensus(),
            &environment(Status::Pending, snapshot),
        )
    };
    let rules_changed = rules(previous) != rules(snapshot);
    let mut conflicts = BTreeMap::new();
    let mut recovery = BTreeSet::new();
    for (hash, entry) in old {
        if facts.attached.contains(hash) {
            continue;
        }
        let Some(value) = entry.accepted() else {
            // Waiting owners retain only missing triggers; their full declared
            // reads must still reject a now-committed spend. Replacement history
            // remains optional recovery work, not a live dependency reader.
            if entry.preaccepted()
                && !facts.spent.is_empty()
                && let Some(point) = entry
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
            continue;
        };
        // Resolution retains inputs in transaction order, before expanded deps.
        // Borrow those identities; only a reported conflict needs an owned key.
        if !facts.spent.is_empty()
            && readers
                .as_ref()
                .is_none_or(|readers| readers.contains(hash))
            && let Some(point) = value
                .transaction
                .resolved_inputs
                .iter()
                .map(|cell| &cell.out_point)
                .chain(value.transaction.related_dep_out_points())
                .find(|point| facts.spent.contains(*point))
        {
            conflicts.insert(hash.clone(), compact_packed(point));
            continue;
        }
        // A pool-resolved cell retains its outpoint identity after its producer
        // commits, including dep-group members. Only a detachment can invalidate
        // that backing, a referenced header or a satisfied time condition.
        let detached_dependency = !facts.detached_headers.is_empty()
            && (value.context_sensitive
                || value
                    .transaction
                    .resolved_inputs
                    .iter()
                    .chain(&value.transaction.resolved_cell_deps)
                    .chain(&value.transaction.resolved_dep_groups)
                    .any(|cell| {
                        facts
                            .detached_transactions
                            .contains(&cell.out_point.tx_hash())
                    })
                || entry
                    .transaction
                    .header_deps_iter()
                    .any(|hash| facts.detached_headers.contains(&hash)));
        if rules_changed || detached_dependency {
            recovery.insert(hash.clone());
        }
    }
    if conflicts.is_empty() && recovery.is_empty() {
        return Ok(Affected {
            conflicts,
            recovery,
        });
    }
    let child_index = membership::children(old);
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
    let mut after: BTreeMap<_, _> = old
        .iter()
        .filter_map(|(hash, entry)| {
            if facts.attached.contains(hash) || affected.conflicts.contains_key(hash) {
                return None;
            }
            let next = if let Some(value) = entry.accepted() {
                if affected.recovery.contains(hash) {
                    Arc::new(Entry {
                        transaction: Arc::clone(&entry.transaction),
                        arrival: entry.arrival,
                        source: Source::Recovery,
                        phase: Phase::Resolve,
                    })
                } else if value
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
                }
            } else {
                let source = current_source(entry, previous, snapshot)?;
                if source != entry.source || matches!(entry.phase, Phase::Verify(_)) {
                    Arc::new(Entry {
                        transaction: Arc::clone(&entry.transaction),
                        arrival: entry.arrival,
                        source,
                        phase: Phase::Resolve,
                    })
                } else {
                    Arc::clone(entry)
                }
            };
            Some((hash.clone(), next))
        })
        .collect();
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

/// Append public phase callbacks in owner order. Both lookup strategies share
/// the phase rule and derive graph totals only if a callback actually needs them.
fn append_proposal_callbacks(
    plan: &mut Plan,
    old: &Members,
    accepted: &Members,
    previous: &ProposalView,
    next: &ProposalView,
    max_ancestors: usize,
) -> Result<(), Error> {
    let mut totals = None;
    let mut append = |hash: &Byte32, entry: &Entry, before, after| -> Result<(), Error> {
        let Some(previous) = old.get(hash).and_then(|entry| entry.accepted()) else {
            return Ok(());
        };
        let value = entry.accepted().ok_or(Error::Stale)?;
        let after = value.status_with(|| after);
        let before = previous.status_with(|| before);
        // Pending and Gap share the public pending callback. Moving between
        // them is not a new admission or another fee-estimator sample.
        if (before == Status::Proposed) == (after == Status::Proposed) {
            return Ok(());
        }
        if totals.is_none() {
            totals = Some(membership::aggregates(accepted, max_ancestors)?);
        }
        let totals = totals
            .as_ref()
            .and_then(|totals| totals.get(hash))
            .ok_or(Error::Stale)?;
        plan.notify(Effect::projected(
            membership::entry_snapshot(entry, *totals)?,
            after,
        ));
        Ok(())
    };
    if let Some(changes) = previous.status_changes(next, accepted.len()) {
        // Full hashes and their proposal prefixes have the same byte ordering.
        // Merge only changed phases without querying both views for each owner.
        let mut remaining = accepted.iter().peekable();
        for (id, before, after) in changes {
            if (before == Status::Proposed) == (after == Status::Proposed) {
                continue;
            }
            while remaining
                .next_if(|(_, entry)| {
                    entry.transaction.proposal_short_id_reader().as_slice() < id.as_slice()
                })
                .is_some()
            {}
            while let Some((hash, entry)) = remaining.next_if(|(_, entry)| {
                entry.transaction.proposal_short_id_reader().as_slice() == id.as_slice()
            }) {
                append(hash, entry, before, after)?;
            }
        }
    } else {
        // Keep sparse-pool work proportional to its owners, even if canonical
        // history contains many unrelated proposal ids.
        let before = previous.status_lookup(accepted.len());
        let after = next.status_lookup(accepted.len());
        for (hash, entry) in accepted {
            let id = entry.transaction.proposal_short_id_reader();
            append(hash, entry, before(id), after(id))?;
        }
    }
    Ok(())
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
        ..
    } = store.capture_all();
    let old: BTreeMap<_, _> = owners
        .into_iter()
        .map(|entry| (entry.hash(), entry))
        .collect();
    let mut facts = ChainFacts::from_blocks(detached_blocks, attached_blocks)?;
    let mut plan = Plan::new(view, Class::Critical, reads);
    let affected = affected_owners(store, &mut plan, &old, &facts, &old_snapshot, snapshot)?;
    let after = successor_population(store, &old, &mut facts, &affected, &old_snapshot, snapshot)?;
    let mut old_totals = None;
    // Compare ordered populations once. Record newly detached owners below,
    // after every old-owner edit and its effects.
    let mut successors = after.iter().peekable();
    for (hash, entry) in &old {
        while successors.next_if(|(next, _)| *next < hash).is_some() {}
        let next = successors
            .next_if(|(next, _)| *next == hash)
            .map(|(_, entry)| entry);
        let effect = if facts.attached.contains(hash) {
            entry.preaccepted_peer().map(|peer| {
                Effect::relay(TxVerificationResult::Ok {
                    original_peer: Some(peer),
                    tx_hash: hash.clone(),
                })
            })
        } else if let Some(point) = affected.conflicts.get(hash) {
            let callback = if entry.accepted().is_some() {
                if old_totals.is_none() {
                    let accepted: Members = old
                        .iter()
                        .filter(|(_, entry)| entry.accepted().is_some())
                        .map(|(hash, entry)| (hash.clone(), Arc::clone(entry)))
                        .collect();
                    old_totals = Some(membership::aggregates(
                        &accepted,
                        config.max_ancestors_count,
                    )?);
                }
                let totals = old_totals
                    .as_ref()
                    .and_then(|totals| totals.get(hash))
                    .ok_or(Error::Stale)?;
                Some(membership::entry_snapshot(entry, *totals)?)
            } else {
                None
            };
            Some(Effect::removed(
                entry,
                Reject::Resolve(OutPointError::Dead(point.clone())),
                callback,
            )?)
        } else {
            None
        };
        if !next.is_some_and(|next| Arc::ptr_eq(next, entry)) {
            plan.edit(Some(Arc::clone(entry)), next.cloned(), effect)?;
        } else if let Some(effect) = effect {
            plan.notify(effect);
        }
    }
    // Rejection effects own their projections; release the old graph scratch
    // before deriving any successor aggregates for status callbacks.
    drop(old_totals);
    // Only detached bodies can introduce hashes absent from the old population.
    if !facts.detached_transactions.is_empty() {
        for (hash, entry) in &after {
            if !old.contains_key(hash) {
                plan.edit(None, Some(Arc::clone(entry)), None)?;
            }
        }
    }
    let mut final_accepted = after;
    final_accepted.retain(|_, entry| entry.accepted().is_some());
    append_proposal_callbacks(
        &mut plan,
        &old,
        &final_accepted,
        old_snapshot.proposals(),
        snapshot.proposals(),
        config.max_ancestors_count,
    )?;
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
