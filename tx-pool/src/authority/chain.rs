//! Reliable chain transitions and pool clears executed with priority preemption
//! and two-phase optimistic parallel planning against `AuthorityState`.
//!
//! Block facts and detached-transaction compaction run off-lock (parallelized
//! across threads for large forks). Reconciliation plans its complete edit set
//! and localized proposal-callback aggregates under a shared read guard so
//! concurrent RPC reads continue uninterrupted, then commits atomically under a
//! microsecond write cutover. If concurrent transaction commits race with the
//! optimistic read plan, reconciliation immediately escalates through
//! `Store::enter_reorg_priority` so chain progress never starves.

use super::{
    jobs::environment,
    membership::{self, Aggregate, NeighborhoodTotals},
    model::{Entry, Error, OutPointKey, Phase, Source, Status, hash_key, proposal_key},
    notice::{Batch, Class, Effect},
    state::{AuthorityState, FastHashMap, FastHashSet, OwnerEdit},
    store::Store,
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
    packed::{Byte32, OutPoint, ProposalShortId},
    prelude::*,
};
use ckb_verification::cache::ScriptVerificationRules;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    sync::Arc,
};

#[cfg(test)]
#[path = "tests/chain.rs"]
mod tests;

const PARALLEL_DETACHED_THRESHOLD: usize = 64;
const PARALLEL_OWNER_SCAN_THRESHOLD: usize = 512;
const MAX_PARALLEL_REORG_THREADS: usize = 4;

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

fn proposal_status_changes(
    state: &AuthorityState,
    next_snapshot: &Snapshot,
) -> Option<Vec<(ProposalShortId, Status, Status)>> {
    state
        .snapshot
        .proposals()
        .status_changes(next_snapshot.proposals(), state.usage.accepted.items)
        .map(Iterator::collect)
}

#[cfg(test)]
thread_local! {
    static INJECT_PARALLEL_WORKER_PANIC_CHUNK: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
struct ParallelPanicInjectionGuard(Option<usize>);

#[cfg(test)]
impl Drop for ParallelPanicInjectionGuard {
    fn drop(&mut self) {
        INJECT_PARALLEL_WORKER_PANIC_CHUNK.with(|slot| slot.set(self.0));
    }
}

#[cfg(test)]
fn inject_parallel_worker_panic(chunk: usize) -> ParallelPanicInjectionGuard {
    let previous = INJECT_PARALLEL_WORKER_PANIC_CHUNK.with(|slot| slot.replace(Some(chunk)));
    ParallelPanicInjectionGuard(previous)
}

fn parallel_filter_map<T: Send, R: Send>(
    items: Vec<T>,
    threshold: usize,
    f: impl Fn(T) -> Option<R> + Sync,
) -> Result<Vec<R>, Error> {
    #[cfg(test)]
    let panic_chunk = INJECT_PARALLEL_WORKER_PANIC_CHUNK.with(std::cell::Cell::get);
    let workers = if items.len() < threshold {
        1
    } else {
        let base = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1)
            .clamp(1, MAX_PARALLEL_REORG_THREADS);
        #[cfg(test)]
        let base = if panic_chunk.is_some() {
            base.max(2)
        } else {
            base
        };
        base
    };
    parallel_filter_map_with_workers(
        items,
        workers,
        #[cfg(test)]
        panic_chunk,
        f,
    )
}

fn parallel_filter_map_with_workers<T: Send, R: Send>(
    mut items: Vec<T>,
    workers: usize,
    #[cfg(test)] panic_chunk: Option<usize>,
    f: impl Fn(T) -> Option<R> + Sync,
) -> Result<Vec<R>, Error> {
    if workers <= 1 {
        return Ok(items.into_iter().filter_map(f).collect());
    }
    let chunk_size = items.len().div_ceil(workers);
    let mut chunks = Vec::with_capacity(workers);
    while !items.is_empty() {
        let split = items.len().saturating_sub(chunk_size);
        chunks.push(items.split_off(split));
    }
    chunks.reverse();
    let f_ref = &f;
    std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .into_iter()
            .enumerate()
            .map(|(chunk_idx, chunk)| {
                scope.spawn(move || {
                    #[cfg(test)]
                    if panic_chunk == Some(chunk_idx) {
                        panic!("injected reorg parallel worker panic in chunk {chunk_idx}");
                    }
                    let _ = chunk_idx;
                    chunk.into_iter().filter_map(f_ref).collect::<Vec<_>>()
                })
            })
            .collect();
        let mut out = Vec::new();
        let mut failed = false;
        for handle in handles {
            match handle.join() {
                Ok(part) if !failed => out.extend(part),
                Ok(_) => {}
                Err(_) => failed = true,
            }
        }
        if failed {
            Err(Error::Fault("reorg parallel worker panicked"))
        } else {
            Ok(out)
        }
    })
}

fn fault_on_err<T>(store: &Store, result: Result<T, Error>) -> Result<T, Error> {
    if matches!(result, Err(Error::Fault(_))) {
        store.fault();
    }
    result
}

fn compact_detached_transactions(
    raw: Vec<TransactionView>,
) -> Result<Vec<Arc<TransactionView>>, Error> {
    parallel_filter_map(raw, PARALLEL_DETACHED_THRESHOLD, |tx| {
        BoundedTransaction::try_new(tx)
            .ok()
            .map(BoundedTransaction::into_transaction)
    })
}

fn extract_detached_txs(
    detached_blocks: &VecDeque<BlockView>,
    attached: &FastHashSet<[u8; 32]>,
) -> Result<(HashSet<Byte32>, Vec<Arc<TransactionView>>), Error> {
    let mut raw_detached = Vec::new();
    let mut detached_transactions = HashSet::new();
    for block in detached_blocks {
        for (index, tx) in block.transactions().into_iter().enumerate() {
            if attached.contains(&hash_key(tx.hash_ref()))
                || !detached_transactions.insert(tx.hash())
            {
                continue;
            }
            if index != 0 {
                raw_detached.push(tx);
            }
        }
    }
    Ok((
        detached_transactions,
        compact_detached_transactions(raw_detached)?,
    ))
}

/// Canonical facts extracted off-lock from a fork transition.
struct ChainFacts {
    attached: FastHashSet<[u8; 32]>,
    spent: FastHashSet<OutPointKey>,
    detached_headers: HashSet<Byte32>,
    detached_transactions: HashSet<Byte32>,
    detached: Vec<Arc<TransactionView>>,
}

impl ChainFacts {
    fn from_blocks(
        detached_blocks: &VecDeque<BlockView>,
        attached_blocks: &VecDeque<BlockView>,
    ) -> Result<Self, Error> {
        let mut attached = FastHashSet::default();
        let mut spent = FastHashSet::default();
        for block in attached_blocks {
            for (index, tx) in block.transactions().iter().enumerate() {
                attached.insert(hash_key(tx.hash_ref()));
                if index != 0 {
                    spent.extend(
                        tx.input_pts_iter()
                            .map(|point| OutPointKey::from_out_point(&point)),
                    );
                }
            }
        }
        let detached_headers = detached_blocks.iter().map(BlockView::hash).collect();
        let (detached_transactions, mut detached) =
            extract_detached_txs(detached_blocks, &attached)?;
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

/// Clear all owners (`ClearScope::All`) or only unaccepted owners (`ClearScope::Unaccepted`).
pub(super) fn commit_clear(
    store: &Store,
    snapshot: Option<Arc<Snapshot>>,
    scope: ClearScope,
) -> Result<Option<Arc<Batch>>, Error> {
    let _priority = store.enter_reorg_priority();
    let (_, batch) = store.commit_reorg(|tx| {
        if tx.state.view == u64::MAX {
            return Err(Error::Fault("view counter"));
        }
        let next_snapshot = snapshot
            .clone()
            .unwrap_or_else(|| Arc::clone(&tx.state.snapshot));
        match scope {
            ClearScope::All => tx.replace_generation(next_snapshot),
            ClearScope::Unaccepted => {
                let edits: Vec<OwnerEdit> = tx
                    .state
                    .owners
                    .iter()
                    .filter(|(_, entry)| entry.accepted().is_none())
                    .map(|(hash, entry)| OwnerEdit::remove(*hash, Arc::clone(entry)))
                    .collect();
                let proposals = proposal_status_changes(tx.state, &next_snapshot);
                tx.commit_edits(edits, Vec::new(), vec![Effect::reset()], Class::Critical)?;
                tx.apply_lifecycle(next_snapshot, proposals.as_deref(), &[]);
                Ok(())
            }
        }
    })?;
    Ok(batch)
}

/// Bounded fallback when detailed reconciliation exceeds pipeline or notice limits.
pub(super) fn commit_recover_bounded(
    store: &Store,
    command: &ChainReorgArgs,
) -> Result<Option<Arc<Batch>>, Error> {
    #[cfg(feature = "profiling")]
    let _span =
        tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.chain.recover").entered();
    let snapshot = command.snapshot();
    let Some(fork) = command.fork() else {
        return commit_clear(store, Some(Arc::clone(snapshot)), ClearScope::All);
    };
    let attached_views: Vec<Arc<BlockView>> = fork
        .attached_blocks
        .iter()
        .map(|block| Arc::new(block.clone()))
        .collect();
    let attached: FastHashSet<[u8; 32]> = fork
        .attached_blocks
        .iter()
        .flat_map(BlockView::tx_hashes)
        .map(hash_key)
        .collect();
    let (_, compacted_detached) = fault_on_err(
        store,
        extract_detached_txs(&fork.detached_blocks, &attached),
    )?;

    let _priority = store.enter_reorg_priority();
    let (_, batch) = store.commit_reorg(|tx| {
        if tx.state.view == u64::MAX {
            return Err(Error::Fault("view counter"));
        }
        let mut candidates: BTreeMap<[u8; 32], (Arc<TransactionView>, u8, u64, Source)> =
            BTreeMap::new();
        for (hash, entry) in &tx.state.owners {
            if attached.contains(hash) {
                continue;
            }
            let Some(source) = (if entry.accepted().is_some() {
                Some(Source::Recovery)
            } else {
                current_source(entry, &tx.state.snapshot, snapshot)
            }) else {
                continue;
            };
            candidates.insert(
                *hash,
                (Arc::clone(&entry.transaction), 0, entry.arrival, source),
            );
        }
        for (idx, transaction) in compacted_detached.iter().enumerate() {
            candidates.insert(
                hash_key(transaction.hash_ref()),
                (Arc::clone(transaction), 1, idx as u64, Source::Recovery),
            );
        }
        let mut ordered: Vec<(Arc<TransactionView>, u8, u64, Source)> =
            candidates.into_values().collect();
        ordered.sort_by_key(|(tx, tier, arrival, source)| {
            (source.priority(), *tier, *arrival, hash_key(tx.hash_ref()))
        });
        crate::dependency_sort::sort_by_dependencies(&mut ordered, |(tx, _, _, _)| tx.as_ref())
            .map_err(|_| Error::Full("recovery dependency ordering".into()))?;
        let mut entries: Vec<Arc<Entry>> = ordered
            .into_iter()
            .map(|(transaction, _, arrival, source)| {
                let arrival = if matches!(source, Source::Recovery) {
                    store.next_arrival()?
                } else {
                    arrival
                };
                Entry::try_new(transaction, arrival, source, Phase::Resolve)
            })
            .collect::<Result<_, _>>()?;
        let mut seen_proposals = FastHashSet::default();
        entries.retain(|entry| {
            !entry
                .preaccepted_peer()
                .is_some_and(|peer| tx.state.is_peer_banned(peer, tx.now))
                && seen_proposals.insert(proposal_key(&entry.hash_bytes()))
        });
        tx.limits.retain_fitting(&mut entries)?;

        let mut retained: FastHashMap<[u8; 32], Arc<Entry>> =
            FastHashMap::with_capacity_and_hasher(entries.len(), Default::default());
        for entry in entries {
            retained.insert(entry.hash_bytes(), entry);
        }
        let mut edits = Vec::with_capacity(tx.state.owners.len().saturating_add(retained.len()));
        for (hash, entry) in &tx.state.owners {
            edits.push(OwnerEdit::new(
                *hash,
                Some(Arc::clone(entry)),
                retained.remove(hash),
            ));
        }
        edits.extend(retained.into_iter().map(|(h, e)| OwnerEdit::insert(h, e)));
        let mut effects = vec![Effect::reset()];
        if !attached_views.is_empty() {
            effects.push(Effect::blocks(attached_views.clone()));
        }
        let proposals = proposal_status_changes(tx.state, snapshot);
        tx.commit_edits(edits, Vec::new(), effects, Class::Critical)?;
        tx.apply_lifecycle(Arc::clone(snapshot), proposals.as_deref(), &attached_views);
        Ok(())
    })?;
    Ok(batch)
}

fn script_rules(snapshot: &Snapshot) -> ScriptVerificationRules {
    ScriptVerificationRules::from_env(
        snapshot.consensus(),
        &environment(Status::Pending, snapshot),
    )
}

fn first_spent_accepted(entry: &Entry, spent: &FastHashSet<OutPointKey>) -> Option<OutPoint> {
    let value = entry.accepted()?;
    value
        .transaction
        .resolved_inputs
        .iter()
        .map(|cell| &cell.out_point)
        .chain(value.transaction.related_dep_out_points())
        .find(|point| spent.contains(&OutPointKey::from_out_point(point)))
        .map(compact_packed)
}

fn owner_needs_recovery(entry: &Entry, facts: &ChainFacts, rules_changed: bool) -> bool {
    let Some(value) = entry.accepted() else {
        return false;
    };
    if rules_changed {
        return true;
    }
    !facts.detached_headers.is_empty()
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
                .any(|header| facts.detached_headers.contains(&header)))
}

fn collect_recovery_roots(
    state: &AuthorityState,
    facts: &ChainFacts,
    conflicts: &BTreeMap<[u8; 32], OutPoint>,
    rules_changed: bool,
) -> Result<FastHashSet<[u8; 32]>, Error> {
    if !rules_changed && facts.detached_headers.is_empty() {
        return Ok(FastHashSet::default());
    }
    let candidates: Vec<([u8; 32], &Entry)> = state
        .owners
        .iter()
        .filter(|(hash, entry)| {
            entry.accepted().is_some()
                && !facts.attached.contains(*hash)
                && !conflicts.contains_key(*hash)
        })
        .map(|(hash, entry)| (*hash, entry.as_ref()))
        .collect();
    Ok(parallel_filter_map(
        candidates,
        PARALLEL_OWNER_SCAN_THRESHOLD,
        |(hash, entry)| owner_needs_recovery(entry, facts, rules_changed).then_some(hash),
    )?
    .into_iter()
    .collect())
}

fn reconcile_existing_owner(
    hash: &[u8; 32],
    entry: &Arc<Entry>,
    attached: &FastHashSet<[u8; 32]>,
    conflicts: &BTreeMap<[u8; 32], OutPoint>,
    recovery: &FastHashSet<[u8; 32]>,
    previous: &Snapshot,
    next_snapshot: &Snapshot,
) -> Result<Option<Arc<Entry>>, Error> {
    if attached.contains(hash) || conflicts.contains_key(hash) {
        return Ok(None);
    }
    if let Some(value) = entry.accepted() {
        if !recovery.contains(hash) {
            if value
                .parents
                .iter()
                .any(|parent| attached.contains(&hash_key(parent)))
            {
                let mut updated = value.clone();
                updated
                    .parents
                    .retain(|parent| !attached.contains(&hash_key(parent)));
                return Ok(Some(entry.try_with_phase(Phase::Accepted(updated))?));
            }
            return Ok(Some(Arc::clone(entry)));
        }
        return Ok(Some(Entry::try_new(
            Arc::clone(&entry.transaction),
            entry.arrival,
            Source::Recovery,
            Phase::Resolve,
        )?));
    }
    let Some(next_source) = current_source(entry, previous, next_snapshot) else {
        return Ok(None);
    };
    if next_source == entry.source && !matches!(entry.phase, Phase::Verify(_)) {
        return Ok(Some(Arc::clone(entry)));
    }
    Ok(Some(Entry::try_new(
        Arc::clone(&entry.transaction),
        entry.arrival,
        next_source,
        Phase::Resolve,
    )?))
}

struct ReconcilePlan {
    base_view: u64,
    base_epoch: u64,
    base_structural_epoch: u64,
    can_tolerate_disjoint_leaves: bool,
    edits: Vec<OwnerEdit>,
    wake_keys: Vec<OutPointKey>,
    effects: Vec<Effect>,
    proposals: Option<Vec<(ProposalShortId, Status, Status)>>,
}

struct ReorgImpacts {
    conflicts: BTreeMap<[u8; 32], OutPoint>,
    recovery: FastHashSet<[u8; 32]>,
}

/// Collect conflicting preaccepted/accepted owners and recovery roots, closing
/// both sets over surviving accepted descendants.
fn collect_conflicts_and_recovery(
    state: &AuthorityState,
    facts: &ChainFacts,
    rules_changed: bool,
) -> Result<ReorgImpacts, Error> {
    let mut conflicts: BTreeMap<[u8; 32], OutPoint> = BTreeMap::new();
    if !facts.spent.is_empty() {
        for hash in &state.preaccepted_owners {
            if !facts.attached.contains(hash)
                && let Some(entry) = state.owner(hash)
                && let Some(point) = entry.find_spent_preaccepted(&facts.spent)
            {
                conflicts.insert(*hash, compact_packed(&point));
            }
        }
        let mut readers = Vec::new();
        for point in &facts.spent {
            readers.clear();
            state.accepted_readers(point, &mut readers);
            for hash in &readers {
                if !facts.attached.contains(hash)
                    && !conflicts.contains_key(hash)
                    && let Some(entry) = state.owner(hash)
                    && let Some(first_point) = first_spent_accepted(entry, &facts.spent)
                {
                    conflicts.insert(*hash, first_point);
                }
            }
        }
    }
    let mut recovery = collect_recovery_roots(state, facts, &conflicts, rules_changed)?;

    let mut conflict_stack: Vec<([u8; 32], OutPoint)> =
        conflicts.iter().map(|(h, p)| (*h, p.clone())).collect();
    while let Some((hash, point)) = conflict_stack.pop() {
        if let Some(children) = state.children.get(&hash) {
            for child in children.iter() {
                if !facts.attached.contains(child) && !conflicts.contains_key(child) {
                    conflicts.insert(*child, point.clone());
                    conflict_stack.push((*child, point.clone()));
                }
            }
        }
    }
    let mut recovery_stack: Vec<[u8; 32]> = recovery.iter().copied().collect();
    while let Some(hash) = recovery_stack.pop() {
        if let Some(children) = state.children.get(&hash) {
            for child in children.iter() {
                if !facts.attached.contains(child)
                    && !conflicts.contains_key(child)
                    && recovery.insert(*child)
                {
                    recovery_stack.push(*child);
                }
            }
        }
    }
    Ok(ReorgImpacts {
        conflicts,
        recovery,
    })
}

fn build_detached_entries(
    state: &AuthorityState,
    store: &Store,
    detached: &[Arc<TransactionView>],
    detached_arrivals: &mut FastHashMap<[u8; 32], u64>,
) -> Result<BTreeMap<[u8; 32], Arc<Entry>>, Error> {
    let mut entries = BTreeMap::new();
    for transaction in detached {
        let hash = hash_key(transaction.hash_ref());
        let arrival = match state.owner(&hash) {
            Some(owner) => owner.arrival,
            None => match detached_arrivals.get(&hash) {
                Some(&arrival) => arrival,
                None => {
                    let next = store.next_arrival()?;
                    detached_arrivals.insert(hash, next);
                    next
                }
            },
        };
        entries.insert(
            hash,
            Entry::try_new(
                Arc::clone(transaction),
                arrival,
                Source::Recovery,
                Phase::Resolve,
            )?,
        );
    }
    Ok(entries)
}

fn collect_block_wake_keys(
    state: &AuthorityState,
    attached: &FastHashSet<[u8; 32]>,
    attached_blocks: &VecDeque<BlockView>,
    detached_blocks: &VecDeque<BlockView>,
) -> Vec<OutPointKey> {
    let mut wake_keys = Vec::new();
    if state.waiters.is_empty() {
        return wake_keys;
    }
    for block in attached_blocks.iter().chain(detached_blocks) {
        for transaction in block.transactions() {
            let hash = hash_key(transaction.hash_ref());
            if attached.contains(&hash)
                && state
                    .owner(&hash)
                    .is_some_and(|entry| entry.accepted().is_some())
            {
                continue;
            }
            for idx in 0..transaction.outputs().len() {
                let key = OutPointKey::from_parts(hash, idx as u32);
                if state.waiters.contains_key(&key) {
                    wake_keys.push(key);
                }
            }
            for point in transaction.input_pts_iter() {
                let key = OutPointKey::from_out_point(&point);
                if state.waiters.contains_key(&key) {
                    wake_keys.push(key);
                }
            }
        }
    }
    wake_keys
}

/// Build the complete reconciliation edit set, localized callbacks and wake keys
/// under a shared read guard so concurrent readers remain unblocked.
#[expect(clippy::too_many_arguments)]
fn plan_reconcile(
    state: &AuthorityState,
    store: &Store,
    facts: &ChainFacts,
    detached_blocks: &VecDeque<BlockView>,
    attached_blocks: &VecDeque<BlockView>,
    attached_views: &[Arc<BlockView>],
    snapshot: &Arc<Snapshot>,
    next_rules: &ScriptVerificationRules,
    config: &TxPoolConfig,
    detached_arrivals: &mut FastHashMap<[u8; 32], u64>,
) -> Result<ReconcilePlan, Error> {
    if state.view == u64::MAX {
        return Err(Error::Fault("view counter"));
    }
    let base_view = state.view;
    let base_epoch = state.mutation_epoch;
    let base_structural_epoch = state.structural_epoch;
    let proposals = proposal_status_changes(state, snapshot);
    let rules_changed = script_rules(&state.snapshot) != *next_rules;
    let can_tolerate_disjoint_leaves = detached_blocks.is_empty()
        && !rules_changed
        && state.preaccepted_owners.is_empty()
        && state.waiters.is_empty();

    let ReorgImpacts {
        conflicts,
        recovery,
    } = collect_conflicts_and_recovery(state, facts, rules_changed)?;
    let mut detached_entries =
        build_detached_entries(state, store, &facts.detached, detached_arrivals)?;

    // Deterministic owner-hash order for edits and public effects.
    let mut touched: BTreeSet<[u8; 32]> = BTreeSet::new();
    for hash in &facts.attached {
        if state.owners.contains_key(hash) {
            touched.insert(*hash);
        }
        if let Some(children) = state.children.get(hash) {
            touched.extend(children.iter().copied());
        }
    }
    touched.extend(conflicts.keys().copied());
    touched.extend(recovery.iter().copied());
    touched.extend(state.preaccepted_owners.iter().copied());
    touched.extend(
        detached_entries
            .keys()
            .filter(|h| state.owners.contains_key(*h))
            .copied(),
    );

    let conflict_accepted: Vec<[u8; 32]> = conflicts
        .keys()
        .copied()
        .filter(|h| state.owner(h).is_some_and(|e| e.accepted().is_some()))
        .collect();
    let conflict_totals = (!conflict_accepted.is_empty())
        .then(|| state.removal_totals(&conflict_accepted, config.max_ancestors_count))
        .transpose()?;

    let mut edits = Vec::with_capacity(touched.len().saturating_add(detached_entries.len()));
    let mut edit_map: FastHashMap<[u8; 32], Option<Arc<Entry>>> =
        FastHashMap::with_capacity_and_hasher(touched.len(), Default::default());
    let mut effects = Vec::new();

    for hash in touched {
        let Some(entry) = state.owner(&hash).cloned() else {
            continue;
        };
        let next = match detached_entries.remove(&hash) {
            Some(detached) => Some(detached),
            None => reconcile_existing_owner(
                &hash,
                &entry,
                &facts.attached,
                &conflicts,
                &recovery,
                &state.snapshot,
                snapshot,
            )?,
        };
        if facts.attached.contains(&hash) {
            if let Some(peer) = entry.preaccepted_peer() {
                effects.push(Effect::relay(TxVerificationResult::Ok {
                    original_peer: Some(peer),
                    tx_hash: entry.hash(),
                }));
            }
        } else if let Some(point) = conflicts.get(&hash) {
            let callback = entry
                .accepted()
                .is_some()
                .then(|| {
                    let totals = conflict_totals
                        .as_ref()
                        .and_then(|map| map.get(&hash))
                        .ok_or(Error::Stale)?;
                    membership::entry_snapshot(&entry, *totals)
                })
                .transpose()?;
            effects.push(Effect::removed(
                &entry,
                Reject::Resolve(OutPointError::Dead(point.clone())),
                callback,
            )?);
        }
        if !next
            .as_ref()
            .is_some_and(|after| Arc::ptr_eq(after, &entry))
        {
            edit_map.insert(hash, next.clone());
            edits.push(OwnerEdit::new(hash, Some(entry), next));
        }
    }

    for (hash, entry) in detached_entries {
        edit_map.insert(hash, Some(Arc::clone(&entry)));
        edits.push(OwnerEdit::insert(hash, entry));
    }

    append_proposal_callbacks(
        state,
        &edit_map,
        proposals.as_deref(),
        snapshot,
        config.max_ancestors_count,
        &mut effects,
    )?;
    if !attached_views.is_empty() {
        effects.push(Effect::blocks(attached_views.to_vec()));
    }

    Ok(ReconcilePlan {
        base_view,
        base_epoch,
        base_structural_epoch,
        can_tolerate_disjoint_leaves,
        edits,
        wake_keys: collect_block_wake_keys(
            state,
            &facts.attached,
            attached_blocks,
            detached_blocks,
        ),
        effects,
        proposals,
    })
}

/// Reconcile live pool state with one authoritative fork transition.
pub(super) fn commit_reconcile(
    store: &Store,
    command: &ChainReorgArgs,
    config: &TxPoolConfig,
) -> Result<Option<Arc<Batch>>, Error> {
    #[cfg(feature = "profiling")]
    let _span =
        tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.chain.reconcile").entered();
    let snapshot = command.snapshot();
    let Some(fork) = command.fork() else {
        return commit_clear(store, Some(Arc::clone(snapshot)), ClearScope::All);
    };
    if fork
        .attached_blocks
        .back()
        .is_some_and(|block| block.hash() != snapshot.tip_hash())
    {
        return Err(Error::Fault("chain snapshot tip"));
    }

    let facts = fault_on_err(
        store,
        ChainFacts::from_blocks(&fork.detached_blocks, &fork.attached_blocks),
    )?;
    let attached_views: Vec<Arc<BlockView>> = fork
        .attached_blocks
        .iter()
        .map(|block| Arc::new(block.clone()))
        .collect();
    let next_rules = script_rules(snapshot);
    let mut detached_arrivals = FastHashMap::default();
    let mut priority_guard = None;

    loop {
        let plan_result = {
            let state = store.open_state_read()?;
            plan_reconcile(
                &state,
                store,
                &facts,
                &fork.detached_blocks,
                &fork.attached_blocks,
                &attached_views,
                snapshot,
                &next_rules,
                config,
                &mut detached_arrivals,
            )
        };
        let plan = fault_on_err(store, plan_result)?;
        let result = store.commit_reorg_releasing(&mut priority_guard, |tx| {
            if tx.state.view == u64::MAX {
                return Err(Error::Fault("view counter"));
            }
            if tx.state.view != plan.base_view {
                return Err(Error::Stale);
            }
            if tx.state.mutation_epoch != plan.base_epoch {
                if !plan.can_tolerate_disjoint_leaves
                    || tx.state.structural_epoch != plan.base_structural_epoch
                    || !tx.state.preaccepted_owners.is_empty()
                    || !tx.state.waiters.is_empty()
                {
                    return Err(Error::Stale);
                }
                let Ok(inserted_count) =
                    usize::try_from(tx.state.mutation_epoch.wrapping_sub(plan.base_epoch))
                else {
                    return Err(Error::Stale);
                };
                let Some(start) = tx
                    .state
                    .recent_leaf_inserts
                    .len()
                    .checked_sub(inserted_count)
                else {
                    return Err(Error::Stale);
                };
                for hash in tx.state.recent_leaf_inserts.range(start..) {
                    let Some(entry) = tx.state.owners.get(hash) else {
                        return Err(Error::Stale);
                    };
                    let Some(accepted) = entry.accepted() else {
                        return Err(Error::Stale);
                    };
                    if !accepted.parents.is_empty()
                        || facts.attached.contains(hash)
                        || accepted.status(&tx.state.snapshot) != accepted.status(snapshot)
                        || first_spent_accepted(entry, &facts.spent).is_some()
                    {
                        return Err(Error::Stale);
                    }
                }
            }
            tx.commit_edits(plan.edits, plan.wake_keys, plan.effects, Class::Critical)?;
            tx.apply_lifecycle(
                Arc::clone(snapshot),
                plan.proposals.as_deref(),
                &attached_views,
            );
            Ok(())
        });
        match result {
            Ok((_, batch)) => return Ok(batch),
            Err(Error::Stale) => {
                if priority_guard.is_none() {
                    priority_guard = Some(store.enter_reorg_priority());
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn surviving_accepted(
    state: &AuthorityState,
    edit_map: &FastHashMap<[u8; 32], Option<Arc<Entry>>>,
    hash: &[u8; 32],
) -> Option<Arc<Entry>> {
    match edit_map.get(hash) {
        Some(after) => after.as_ref().filter(|e| e.accepted().is_some()).cloned(),
        None => state
            .owners
            .get(hash)
            .filter(|e| e.accepted().is_some())
            .cloned(),
    }
}

/// Collect only the surviving accepted connected component(s) reachable from
/// `roots`, turning proposal-callback aggregation from O(|pool|) into O(|component|).
fn collect_surviving_component(
    state: &AuthorityState,
    edit_map: &FastHashMap<[u8; 32], Option<Arc<Entry>>>,
    roots: impl Iterator<Item = ([u8; 32], Arc<Entry>)>,
) -> Vec<Arc<Entry>> {
    let mut seen = FastHashSet::default();
    let mut component = Vec::new();
    let mut stack = Vec::new();
    for (hash, entry) in roots {
        if seen.insert(hash) {
            stack.push((hash, entry));
        }
    }
    while let Some((hash, entry)) = stack.pop() {
        let parent_keys = entry
            .accepted()
            .into_iter()
            .flat_map(|v| v.parents.iter().map(hash_key));
        let child_keys = state
            .children
            .get(&hash)
            .into_iter()
            .flat_map(|c| c.iter().copied());
        for neighbor in parent_keys.chain(child_keys) {
            if !seen.contains(&neighbor)
                && let Some(neighbor_entry) = surviving_accepted(state, edit_map, &neighbor)
                && seen.insert(neighbor)
            {
                stack.push((neighbor, neighbor_entry));
            }
        }
        component.push(entry);
    }
    component
}

/// Append public Pending <-> Proposed callbacks in deterministic owner-hash order.
fn append_proposal_callbacks(
    state: &AuthorityState,
    edit_map: &FastHashMap<[u8; 32], Option<Arc<Entry>>>,
    proposals: Option<&[(ProposalShortId, Status, Status)]>,
    next_snapshot: &Snapshot,
    max_ancestors: usize,
    effects: &mut Vec<Effect>,
) -> Result<(), Error> {
    let mut transitions: Vec<([u8; 32], Arc<Entry>, Status)> = Vec::new();
    let mut record = |hash: [u8; 32], before: Status, after: Status| {
        if let Some(entry) = surviving_accepted(state, edit_map, &hash)
            && let Some(value) = entry.accepted()
        {
            let eff_before = value.status_with(|| before);
            let eff_after = value.status_with(|| after);
            if (eff_before == Status::Proposed) != (eff_after == Status::Proposed) {
                transitions.push((hash, entry, eff_after));
            }
        }
    };
    if let Some(changes) = proposals {
        for (id, before, after) in changes {
            if (*before == Status::Proposed) != (*after == Status::Proposed)
                && let Some(original) = state.proposal_owner(&id.unpack())
            {
                record(original.hash_bytes(), *before, *after);
            }
        }
    } else {
        let count = state.usage.accepted.items;
        let before = state.snapshot.proposals().status_lookup(count);
        let after = next_snapshot.proposals().status_lookup(count);
        for (hash, original) in &state.owners {
            if original.accepted().is_some() {
                let id = original.transaction.proposal_short_id_reader();
                record(*hash, before(id), after(id));
            }
        }
    }
    if transitions.is_empty() {
        return Ok(());
    }
    transitions.sort_unstable_by_key(|(hash, _, _)| *hash);

    let connected_roots: Vec<([u8; 32], Arc<Entry>)> = transitions
        .iter()
        .filter(|(hash, entry, _)| {
            entry.accepted().is_some_and(|v| !v.parents.is_empty())
                || state.children.get(hash).is_some_and(|children| {
                    children
                        .iter()
                        .any(|child| surviving_accepted(state, edit_map, child).is_some())
                })
        })
        .map(|(hash, entry, _)| (*hash, Arc::clone(entry)))
        .collect();

    let component_totals = (!connected_roots.is_empty())
        .then(|| {
            let component =
                collect_surviving_component(state, edit_map, connected_roots.into_iter());
            membership::aggregates(component.iter(), max_ancestors)
        })
        .transpose()?;

    for (hash, entry, after_status) in transitions {
        let value = entry.accepted().ok_or(Error::Stale)?;
        let totals = component_totals
            .as_ref()
            .and_then(|map| map.get(&hash))
            .copied()
            .unwrap_or_else(|| {
                let own = Aggregate::one(value);
                NeighborhoodTotals {
                    ancestors: own,
                    descendants: own,
                }
            });
        effects.push(Effect::projected(
            membership::entry_snapshot(&entry, totals)?,
            after_status,
        ));
    }
    Ok(())
}
