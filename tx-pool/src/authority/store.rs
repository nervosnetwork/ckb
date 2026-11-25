//! Single-writer transaction-pool store, priority-aware lock boundary and post-lock publication.
//!
//! `Store` pairs one short-hold `RwLock<AuthorityState>` with a `ReorgGate`
//! priority barrier, `ActiveBudget`, runnable work `Queues`, and the ordered
//! `Outbox`. All I/O, VM verification, template construction, callbacks and
//! payload drops happen outside the lock.

use super::{
    budget::{ActiveBudget, Limits},
    jobs::Job,
    model::{Entry, Error, OutPointKey, Premises, Status, hash_key},
    notice::{Batch, Class, Effect, Outbox},
    queue::{Queues, WorkSelection, WorkStage},
    ranks::RankedPriorities,
    state::{AuthorityState, CommitOutcome, EditBatch, FastHashMap, OwnerEdit, WAKE_PAGE},
    template::TemplateSource,
};
use crate::util::compact_packed;
use ckb_app_config::TxPoolConfig;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{
    bytes::Bytes,
    core::{
        BlockView,
        cell::{CellMeta, CellMetaBuilder},
    },
    packed::{Byte32, CellOutput, OutPoint, ProposalShortId},
    prelude::*,
};
use ckb_util::parking_lot::{Condvar, Mutex, RwLock, RwLockReadGuard};
use lru::LruCache;
use std::{
    ops::Bound::{Excluded, Unbounded},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;

#[cfg(test)]
#[path = "tests/concurrency.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/state_transitions.rs"]
mod transition_tests;

/// Single-lock batched read observation for transaction resolution.
pub(super) struct ResolutionObservation {
    pub(super) view: u64,
    pub(super) snapshot: Arc<Snapshot>,
    pub(super) before: Option<Arc<Entry>>,
    pub(super) prefetched: FastHashMap<OutPointKey, Option<Arc<Entry>>>,
}

/// Copy one accepted pool cell output/data within `byte_limit` outside any lock.
pub(super) fn materialize_pool_cell(
    owner: &Entry,
    point: &OutPoint,
    index: u32,
    byte_limit: usize,
) -> Result<Option<CellMeta>, Error> {
    let Some((output, data)) = owner.transaction.output_with_data(index as usize) else {
        return Ok(None);
    };
    let bytes = output
        .total_size()
        .checked_add(data.len())
        .and_then(|b| b.checked_add(256))
        .ok_or(Error::Full("pool cell arithmetic".into()))?;
    if bytes > byte_limit {
        return Err(Error::Full("pool cell materialization".into()));
    }
    Ok(Some(
        CellMetaBuilder::from_cell_output(
            CellOutput::new_unchecked(Bytes::copy_from_slice(output.as_slice())),
            Bytes::copy_from_slice(&data),
        )
        .out_point(compact_packed(point))
        .build(),
    ))
}

fn observe_point(
    state: &AuthorityState,
    key: OutPointKey,
    premises: &mut Premises,
) -> Result<Option<Arc<Entry>>, Error> {
    premises.observe_spender_key(key, state.spenders.get(&key).copied())?;
    let tx_hash = *key.tx_hash();
    let owner = state.owners.get(&tx_hash);
    premises.observe_owner_key(tx_hash, owner)?;
    Ok(owner.filter(|entry| entry.accepted().is_some()).cloned())
}

/// Coherent snapshot of owners without retaining any authority lock.
pub(super) struct Captured {
    pub(super) view: u64,
    pub(super) snapshot: Arc<Snapshot>,
    pub(super) owners: Vec<Arc<Entry>>,
    pub(super) accepted_revision: u64,
    pub(super) accepted_removal_revision: u64,
    /// True when `AuthorityState` already guarantees every accepted owner has
    /// disjoint inputs, zero in-pool parents, and zero read-before-spend edges.
    pub(super) independent_leaves: bool,
}

/// Snapshot of one dirty dependency's wake page for off-lock DB checks.
pub(super) struct WakePageSnapshot {
    pub(super) view: u64,
    pub(super) snapshot: Arc<Snapshot>,
    pub(super) key: OutPointKey,
    pub(super) pass: u64,
    pub(super) after: Option<[u8; 32]>,
    pub(super) entries: Vec<Arc<Entry>>,
}

#[cfg(test)]
impl WakePageSnapshot {
    pub(super) fn key(&self) -> OutPointKey {
        self.key
    }

    pub(super) fn pass(&self) -> u64 {
        self.pass
    }

    pub(super) fn hashes(&self) -> Vec<[u8; 32]> {
        self.entries
            .iter()
            .map(|entry| entry.hash_bytes())
            .collect()
    }
}

/// Priority preemption barrier ensuring authoritative chain transitions never
/// starve behind ordinary transaction floods or heavy full-pool scans.
#[derive(Default)]
struct ReorgGate {
    pending: AtomicUsize,
    #[cfg(test)]
    parked: AtomicUsize,
    wait: Mutex<()>,
    clear: Condvar,
}

impl ReorgGate {
    fn wait_if_pending(&self) {
        if self.pending.load(Ordering::SeqCst) == 0 {
            return;
        }
        crate::metrics::record_reorg_gate_wait();
        let mut guard = self.wait.lock();
        while self.pending.load(Ordering::SeqCst) > 0 {
            #[cfg(test)]
            self.parked.fetch_add(1, Ordering::SeqCst);
            self.clear.wait(&mut guard);
            #[cfg(test)]
            self.parked.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn enter(&self) -> ReorgPriorityGuard<'_> {
        self.pending.fetch_add(1, Ordering::SeqCst);
        ReorgPriorityGuard { gate: self }
    }
}

pub(super) struct ReorgPriorityGuard<'a> {
    gate: &'a ReorgGate,
}

impl Drop for ReorgPriorityGuard<'_> {
    fn drop(&mut self) {
        if self.gate.pending.fetch_sub(1, Ordering::SeqCst) == 1 {
            // Take the gate mutex before notifying so a waiter between checking
            // `pending > 0` and entering `clear.wait` cannot miss the wake.
            let _guard = self.gate.wait.lock();
            self.gate.clear.notify_all();
        }
    }
}

/// Exclusive single-writer transaction context inside [`Store::commit_with`].
/// Enforces `preflight_edits -> outbox.commit -> apply_edits` on every mutation.
pub(super) struct Commit<'a> {
    pub(super) state: &'a mut AuthorityState,
    pub(super) limits: &'a Limits,
    pub(super) now: Instant,
    queues: &'a Queues,
    outbox: &'a Arc<Outbox>,
    budget_cooldown: &'a TimeBudgetCooldown,
    batch: Option<Arc<Batch>>,
    outcome: CommitOutcome,
}

impl Commit<'_> {
    fn reserve_effects(&mut self, effects: Vec<Effect>, class: Class) -> Result<(), Error> {
        if effects.is_empty() {
            return Ok(());
        }
        if self.batch.is_some() {
            return Err(Error::Fault("duplicate commit notice batch"));
        }
        self.batch = self.outbox.commit(effects, class)?;
        Ok(())
    }

    fn commit_batch(
        &mut self,
        edits: EditBatch,
        wake_keys: Vec<OutPointKey>,
        effects: Vec<Effect>,
        class: Class,
    ) -> Result<(), Error> {
        self.state
            .preflight_edits(edits.as_slice(), self.limits, self.now, &wake_keys)?;
        self.reserve_effects(effects, class)?;
        if !edits.is_empty() || !wake_keys.is_empty() {
            self.state
                .apply_edits(edits, wake_keys, self.queues, &mut self.outcome)?;
        }
        Ok(())
    }

    pub(super) fn commit_edits(
        &mut self,
        edits: Vec<OwnerEdit>,
        wake_keys: Vec<OutPointKey>,
        effects: Vec<Effect>,
        class: Class,
    ) -> Result<(), Error> {
        self.commit_batch(EditBatch::Many(edits), wake_keys, effects, class)
    }

    pub(super) fn commit_single(
        &mut self,
        hash: [u8; 32],
        before: Option<Arc<Entry>>,
        after: Option<Arc<Entry>>,
        effects: Vec<Effect>,
        class: Class,
    ) -> Result<(), Error> {
        self.commit_single_with_ancestors(OwnerEdit::new(hash, before, after), effects, class)
    }

    pub(super) fn commit_single_with_ancestors(
        &mut self,
        edit: OwnerEdit,
        effects: Vec<Effect>,
        class: Class,
    ) -> Result<(), Error> {
        self.commit_batch(EditBatch::One(Some(edit)), Vec::new(), effects, class)
    }

    pub(super) fn commit_effects(
        &mut self,
        effects: Vec<Effect>,
        class: Class,
    ) -> Result<(), Error> {
        self.reserve_effects(effects, class)
    }

    pub(super) fn apply_lifecycle(
        &mut self,
        next_snapshot: Arc<Snapshot>,
        proposals: Option<&[(ProposalShortId, Status, Status)]>,
        attached_blocks: &[Arc<BlockView>],
    ) {
        self.state
            .apply_lifecycle(next_snapshot, proposals, attached_blocks, &mut self.outcome);
    }

    pub(super) fn replace_generation(&mut self, next_snapshot: Arc<Snapshot>) -> Result<(), Error> {
        self.reserve_effects(vec![Effect::reset()], Class::Critical)?;
        self.state
            .replace_generation(next_snapshot, self.queues, &mut self.outcome);
        Ok(())
    }

    pub(super) fn advance_wake(&mut self, key: &OutPointKey, pass: u64, after: Option<[u8; 32]>) {
        self.state.advance_wake(key, pass, after);
    }

    pub(super) fn apply_ban(&mut self, peer: PeerIndex, deadline: Instant) {
        self.state.apply_ban(peer, deadline, self.now);
    }

    pub(super) fn record_budget_cooldown(&mut self, tx_hash: [u8; 32], witness_hash: [u8; 32]) {
        self.budget_cooldown.record(tx_hash, witness_hash, self.now);
    }

    pub(super) fn is_budget_cooled_down(&self, tx_hash: [u8; 32], witness_hash: [u8; 32]) -> bool {
        self.budget_cooldown
            .is_active(tx_hash, witness_hash, self.now)
    }
}

#[cfg(test)]
type BeforeCommitHook = Arc<dyn Fn() + Send + Sync>;

#[cfg(any(test, feature = "internal"))]
pub type VerifyObserverHook = Arc<dyn Fn(&Byte32) + Send + Sync>;

/// Acquire a `parking_lot::RwLock` read guard bounded by `deadline`.
///
/// Uncontended reads return immediately without offloading; contended reads
/// offload and wait in bounded slices so shutdown, faults and deadline expiry
/// (including paused Tokio clocks) are observed promptly.
pub(super) fn read_rwlock_until<T>(
    lock: &RwLock<T>,
    deadline: tokio::time::Instant,
    check_open: impl Fn() -> Result<(), Error>,
) -> Result<RwLockReadGuard<'_, T>, Error> {
    if let Some(guard) = lock.try_read() {
        check_open()?;
        return Ok(guard);
    }
    crate::util::block_offload(|| {
        loop {
            check_open()?;
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(Error::Full("template refresh timeout".into()));
            }
            let slice = remaining.min(std::time::Duration::from_millis(5));
            if let Some(guard) = lock.try_read_for(slice) {
                check_open()?;
                return Ok(guard);
            }
        }
    })
}

const BUDGET_COOLDOWN_CAPACITY: usize = 1_024;
const BUDGET_COOLDOWN_TTL: Duration = Duration::from_secs(30);

/// Bounded, non-persistent short-TTL cooldown for remote transactions that
/// recently exhausted their VM CPU-time budget.
///
/// Keyed by `(tx_hash, witness_hash)` so re-signed or fixed witnesses, local
/// RPC submissions, recovery, and block proposals are never blocked. Cleared
/// automatically on chain tip transitions and generation resets.
type CooldownKey = ([u8; 32], [u8; 32]);

struct TimeBudgetCooldown {
    non_empty: AtomicBool,
    entries: Mutex<LruCache<CooldownKey, Instant>>,
}

impl TimeBudgetCooldown {
    fn new() -> Self {
        Self {
            non_empty: AtomicBool::new(false),
            entries: Mutex::new(LruCache::new(BUDGET_COOLDOWN_CAPACITY)),
        }
    }

    fn record(&self, tx_hash: [u8; 32], witness_hash: [u8; 32], now: Instant) {
        let Some(deadline) = now.checked_add(BUDGET_COOLDOWN_TTL) else {
            return;
        };
        let mut entries = self.entries.lock();
        entries.put((tx_hash, witness_hash), deadline);
        self.non_empty.store(true, Ordering::SeqCst);
    }

    fn is_active(&self, tx_hash: [u8; 32], witness_hash: [u8; 32], now: Instant) -> bool {
        if !self.non_empty.load(Ordering::SeqCst) {
            return false;
        }
        let mut entries = self.entries.lock();
        let key = (tx_hash, witness_hash);
        match entries.peek(&key).copied() {
            Some(deadline) if now < deadline => true,
            Some(_) => {
                entries.pop(&key);
                if entries.is_empty() {
                    self.non_empty.store(false, Ordering::SeqCst);
                }
                false
            }
            None => false,
        }
    }

    fn clear(&self) {
        if !self.non_empty.load(Ordering::SeqCst) {
            return;
        }
        let mut entries = self.entries.lock();
        entries.clear();
        self.non_empty.store(false, Ordering::SeqCst);
    }
}

/// Lock order (strictly 2 levels):
///   1. `state: RwLock<AuthorityState>`
///   2. Leaf bookkeeping mutexes (`queues`, `outbox.state`, `budget.active`, `budget_cooldown`)
pub(super) struct Store {
    #[cfg(test)]
    pub(super) admission_attempts: AtomicU64,
    #[cfg(test)]
    pub(super) before_commit: Mutex<Option<BeforeCommitHook>>,
    #[cfg(any(test, feature = "internal"))]
    pub(super) verify_observer: Mutex<Option<VerifyObserverHook>>,
    #[cfg(test)]
    pub(super) preview_observer: Mutex<Option<VerifyObserverHook>>,
    state: RwLock<AuthorityState>,
    reorg: ReorgGate,
    arrival: AtomicU64,
    stopped: AtomicBool,
    faulted: Arc<AtomicBool>,
    pub(super) budget: Arc<ActiveBudget>,
    pub(super) outbox: Arc<Outbox>,
    queues: Queues,
    budget_cooldown: TimeBudgetCooldown,
    pub(super) changed: Notify,
    pub(super) template_changed: Notify,
    pub(super) work: Notify,
}

impl Store {
    pub(super) fn new(snapshot: Arc<Snapshot>, config: &TxPoolConfig) -> Result<Arc<Self>, Error> {
        let limits = Limits::new(config, snapshot.consensus())?;
        Self::with_limits(snapshot, config, limits)
    }

    pub(super) fn with_limits(
        snapshot: Arc<Snapshot>,
        config: &TxPoolConfig,
        limits: Limits,
    ) -> Result<Arc<Self>, Error> {
        let faulted = Arc::new(AtomicBool::new(false));
        let outbox = Outbox::new(&limits, Arc::clone(&faulted))?;
        let budget = ActiveBudget::new(limits, Arc::clone(&faulted));
        Ok(Arc::new(Self {
            #[cfg(test)]
            admission_attempts: AtomicU64::new(0),
            #[cfg(test)]
            before_commit: Mutex::new(None),
            #[cfg(any(test, feature = "internal"))]
            verify_observer: Mutex::new(None),
            #[cfg(test)]
            preview_observer: Mutex::new(None),
            state: RwLock::new(AuthorityState::new(snapshot)),
            reorg: ReorgGate::default(),
            arrival: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
            faulted,
            budget,
            outbox,
            queues: Queues::new(config.verify_ordering, config.max_tx_verify_cycles),
            budget_cooldown: TimeBudgetCooldown::new(),
            changed: Notify::new(),
            template_changed: Notify::new(),
            work: Notify::new(),
        }))
    }

    pub(super) fn state_read(&self) -> RwLockReadGuard<'_, AuthorityState> {
        self.state.read()
    }

    pub(super) fn open_state_read(&self) -> Result<RwLockReadGuard<'_, AuthorityState>, Error> {
        let state = self.state.read();
        if self.is_faulted() {
            return Err(Error::Fault("closed generation"));
        }
        Ok(state)
    }

    /// Acquire `state.read()` bounded by `deadline`, checking fault/stop and
    /// virtual/wall-clock expiry if a writer holds the lock.
    fn open_state_read_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<RwLockReadGuard<'_, AuthorityState>, Error> {
        read_rwlock_until(&self.state, deadline, || {
            if self.is_faulted() {
                Err(Error::Fault("closed generation"))
            } else if self.is_stopped() {
                Err(Error::Closed)
            } else {
                Ok(())
            }
        })
    }

    #[cfg(test)]
    pub(super) fn state_write(
        &self,
    ) -> ckb_util::parking_lot::RwLockWriteGuard<'_, AuthorityState> {
        self.state.write()
    }

    pub(super) fn enter_reorg_priority(&self) -> ReorgPriorityGuard<'_> {
        self.reorg.enter()
    }

    #[cfg(test)]
    pub(super) fn reorg_pending(&self) -> bool {
        self.reorg.pending.load(Ordering::SeqCst) > 0
    }

    #[cfg(test)]
    pub(super) fn reorg_parked(&self) -> usize {
        self.reorg.parked.load(Ordering::SeqCst)
    }

    pub(super) fn commit_with<T>(
        &self,
        operation: impl FnOnce(&mut Commit<'_>) -> Result<T, Error>,
    ) -> Result<(T, Option<Arc<Batch>>), Error> {
        self.reorg.wait_if_pending();
        self.commit_inner(operation, None, false, false)
    }

    /// Like [`Store::commit_with`], but activates any committed notice batch via
    /// [`Batch::activate_nonblocking`] so callback-free in-memory effects settle
    /// inline without waking `TxPoolPublisher`.
    pub(super) fn commit_with_inline_publish<T>(
        &self,
        operation: impl FnOnce(&mut Commit<'_>) -> Result<T, Error>,
    ) -> Result<(T, Option<Arc<Batch>>), Error> {
        self.reorg.wait_if_pending();
        self.commit_inner(operation, None, false, true)
    }

    /// Execute a read-only admission preview under `state.read()`, never
    /// taking `state.write()`. Preserves reorg priority and fault propagation.
    pub(super) fn preview_with<T>(
        &self,
        operation: impl FnOnce(&AuthorityState, &Limits, Instant) -> Result<T, Error>,
    ) -> Result<T, Error> {
        #[cfg(feature = "profiling")]
        let _span =
            tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.authority.preview")
                .entered();
        if self.is_faulted() {
            return Err(Error::Fault("closed generation"));
        }
        self.reorg.wait_if_pending();
        #[cfg(test)]
        self.run_before_commit();
        let result = {
            let state = self.open_state_read()?;
            operation(&state, &self.budget.limits, Instant::now())
        };
        if matches!(result, Err(Error::Fault(_))) {
            self.fault();
        }
        result
    }

    pub(super) fn commit_reorg<T>(
        &self,
        operation: impl FnOnce(&mut Commit<'_>) -> Result<T, Error>,
    ) -> Result<(T, Option<Arc<Batch>>), Error> {
        self.commit_inner(operation, None, true, false)
    }

    pub(super) fn commit_reorg_releasing<T>(
        &self,
        priority_guard: &mut Option<ReorgPriorityGuard<'_>>,
        operation: impl FnOnce(&mut Commit<'_>) -> Result<T, Error>,
    ) -> Result<(T, Option<Arc<Batch>>), Error> {
        self.commit_inner(operation, Some(priority_guard), true, false)
    }

    #[cfg(test)]
    pub(super) fn record_budget_cooldown(&self, tx_hash: [u8; 32], witness_hash: [u8; 32]) {
        self.budget_cooldown
            .record(tx_hash, witness_hash, Instant::now());
    }

    pub(super) fn is_budget_cooled_down(
        &self,
        tx_hash: [u8; 32],
        witness_hash: [u8; 32],
        now: Instant,
    ) -> bool {
        self.budget_cooldown.is_active(tx_hash, witness_hash, now)
    }

    pub(super) fn pace_peer(&self, peer: PeerIndex, delay: Duration) {
        self.queues.pace_peer(peer, delay);
    }

    fn commit_inner<T>(
        &self,
        operation: impl FnOnce(&mut Commit<'_>) -> Result<T, Error>,
        priority_guard: Option<&mut Option<ReorgPriorityGuard<'_>>>,
        clear_cooldown: bool,
        inline_publish: bool,
    ) -> Result<(T, Option<Arc<Batch>>), Error> {
        #[cfg(feature = "profiling")]
        let _span = tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.authority.apply")
            .entered();
        if self.is_faulted() {
            return Err(Error::Fault("closed generation"));
        }
        #[cfg(test)]
        self.run_before_commit();
        let write_wait_start = ckb_metrics::handle().is_some().then(Instant::now);
        let mut write_hold_start = None;
        let mut descendants_len = 0usize;
        let result = {
            let mut state = self.state.write();
            if write_wait_start.is_some() {
                write_hold_start = Some(Instant::now());
            }
            if self.is_faulted() {
                return Err(Error::Fault("closed generation"));
            }
            let mut tx = Commit {
                state: &mut state,
                limits: &self.budget.limits,
                now: Instant::now(),
                queues: &self.queues,
                outbox: &self.outbox,
                budget_cooldown: &self.budget_cooldown,
                batch: None,
                outcome: CommitOutcome::default(),
            };
            let committed = operation(&mut tx).map(|value| (value, tx.batch, tx.outcome));
            if clear_cooldown && committed.is_ok() {
                // Clear cooldown inside `state.write()` so view transitions and
                // same-view cooldown writes serialize atomically.
                self.budget_cooldown.clear();
            }
            if write_wait_start.is_some() {
                descendants_len = state.descendants.len();
            }
            committed
        };
        if result.is_ok()
            && let Some(guard) = priority_guard
        {
            // Release ReorgGate immediately after unlocking `state.write()` so
            // writers resume before post-lock outbox activation and retired
            // payload deallocation.
            drop(guard.take());
        }
        if let (Some(wait_start), Some(hold_start)) = (write_wait_start, write_hold_start) {
            let hold_end = Instant::now();
            crate::metrics::record_authority_write(
                hold_start.saturating_duration_since(wait_start),
                hold_end.saturating_duration_since(hold_start),
                descendants_len,
            );
        }
        let (value, batch, outcome) = match result {
            Ok(committed) => committed,
            Err(error) => {
                if matches!(error, Error::Fault(_)) {
                    self.fault();
                }
                return Err(error);
            }
        };
        if let Some(batch) = &batch {
            if inline_publish {
                batch.activate_nonblocking(&self.outbox);
            } else {
                batch.activate(&self.outbox);
            }
        }
        if outcome.template_changed {
            self.template_changed.notify_waiters();
        }
        if outcome.maintenance_ready {
            self.changed.notify_waiters();
        }
        if outcome.work_ready {
            self.work.notify_waiters();
        }
        drop(outcome.retired);
        if self.is_faulted() {
            return Err(Error::Fault("committed projection"));
        }
        Ok((value, batch))
    }

    pub(super) fn snapshot(&self) -> (u64, Arc<Snapshot>) {
        let state = self.state.read();
        (state.view, Arc::clone(&state.snapshot))
    }

    pub(super) fn next_arrival(&self) -> Result<u64, Error> {
        self.arrival
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| old.checked_add(1))
            .map_err(|_| {
                self.fault();
                Error::Fault("arrival counter")
            })
    }

    pub(super) fn fault(&self) {
        if !self.faulted.swap(true, Ordering::SeqCst) {
            crate::metrics::record_failure(crate::metrics::FailureBoundary::TypedFault);
        }
        self.changed.notify_waiters();
        self.template_changed.notify_waiters();
        self.work.notify_waiters();
        self.outbox.failed.notify_waiters();
        self.outbox.changed.notify_waiters();
        self.outbox.notify_publisher();
        self.budget.active_changed.notify_waiters();
    }

    pub(super) fn is_faulted(&self) -> bool {
        self.faulted.load(Ordering::SeqCst)
    }

    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.work.notify_waiters();
        self.changed.notify_waiters();
        self.template_changed.notify_waiters();
    }

    pub(super) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub(super) fn point(&self, hash: &Byte32) -> (Arc<Snapshot>, Option<Arc<Entry>>) {
        let state = self.state.read();
        (
            Arc::clone(&state.snapshot),
            state.owners.get(&hash_key(hash)).cloned(),
        )
    }

    pub(super) fn is_current(&self, entry: &Arc<Entry>) -> bool {
        self.state
            .read()
            .owners
            .get(&entry.hash_bytes())
            .is_some_and(|current| Arc::ptr_eq(current, entry))
    }

    pub(super) fn get(
        &self,
        hash: &Byte32,
        premises: &mut Premises,
    ) -> Result<Option<Arc<Entry>>, Error> {
        let entry = self.state.read().owners.get(&hash_key(hash)).cloned();
        premises.observe_owner(hash, entry.as_ref())?;
        Ok(entry)
    }

    /// Observe chain view, snapshot, optional own entry, and every prefetched
    /// outpoint's spender/producer under one single short `state.read()`.
    pub(super) fn observe_resolution(
        &self,
        own_hash: Option<&Byte32>,
        points: &[OutPointKey],
        premises: &mut Premises,
    ) -> Result<ResolutionObservation, Error> {
        let mut prefetched =
            FastHashMap::with_capacity_and_hasher(points.len(), Default::default());
        premises.reserve(points.len());
        let state = self.state.read();
        let view = state.view;
        let snapshot = Arc::clone(&state.snapshot);
        let before = own_hash.and_then(|hash| state.owners.get(&hash_key(hash)).cloned());
        if before.as_ref().is_none_or(|e| e.accepted().is_none()) {
            for &key in points {
                let owner = observe_point(&state, key, premises)?;
                prefetched.insert(key, owner);
            }
        }
        Ok(ResolutionObservation {
            view,
            snapshot,
            before,
            prefetched,
        })
    }

    pub(super) fn observe_point_premises(
        &self,
        key: OutPointKey,
        premises: &mut Premises,
    ) -> Result<Option<Arc<Entry>>, Error> {
        observe_point(&self.state.read(), key, premises)
    }

    /// Observe spender/producer under one short read lock, then copy detached payload off-lock.
    pub(super) fn resolve_pool_cell(
        &self,
        point: &OutPoint,
        byte_limit: usize,
        premises: &mut Premises,
    ) -> Result<Option<CellMeta>, Error> {
        let key = OutPointKey::from_out_point(point);
        let Some(owner) = self.observe_point_premises(key, premises)? else {
            return Ok(None);
        };
        materialize_pool_cell(&owner, point, key.index(), byte_limit)
    }

    pub(super) fn check_premises(&self, view: u64, premises: &Premises) -> Result<(), Error> {
        self.open_state_read()?.validate_premises(view, premises)
    }

    pub(super) fn validate_preview_premises(
        &self,
        view: u64,
        premises: &Premises,
    ) -> Result<(), Error> {
        #[cfg(test)]
        self.run_before_commit();
        self.check_premises(view, premises)
    }

    pub(super) fn snapshot_checked(
        &self,
        view: u64,
        premises: &Premises,
    ) -> Result<Arc<Snapshot>, Error> {
        let state = self.open_state_read()?;
        state.validate_premises(view, premises)?;
        Ok(Arc::clone(&state.snapshot))
    }

    pub(super) fn check_accepted_revision(
        &self,
        view: u64,
        accepted_revision: u64,
    ) -> Result<(), Error> {
        let state = self.open_state_read()?;
        if state.view == view && state.accepted_revision == accepted_revision {
            Ok(())
        } else {
            Err(Error::Stale)
        }
    }

    /// Validate a `TemplateSource` under one short read guard and run `publish`.
    /// O(1) when `accepted_removal_revision` is unchanged since template capture.
    pub(super) fn read_template<R>(
        &self,
        source: &TemplateSource,
        publish: impl FnOnce() -> R,
    ) -> Result<R, Error> {
        let state = self.open_state_read()?;
        Self::validate_template_source(&state, source)?;
        Ok(publish())
    }

    /// Validate a `TemplateSource` bounded by `deadline` so cache validation
    /// cannot hang past the request deadline when a writer holds `state`.
    pub(super) fn read_template_until<R>(
        &self,
        source: &TemplateSource,
        deadline: tokio::time::Instant,
        publish: impl FnOnce() -> R,
    ) -> Result<R, Error> {
        let state = self.open_state_read_until(deadline)?;
        Self::validate_template_source(&state, source)?;
        Ok(publish())
    }

    fn validate_template_source(
        state: &AuthorityState,
        source: &TemplateSource,
    ) -> Result<(), Error> {
        if state.view != source.view {
            return Err(Error::Stale);
        }
        if state.accepted_removal_revision != source.removal_revision {
            for (hash, expected) in &source.selected_owners {
                if state
                    .owners
                    .get(hash)
                    .is_none_or(|current| !std::ptr::eq(expected.as_ptr(), Arc::as_ptr(current)))
                {
                    return Err(Error::Stale);
                }
            }
        }
        Ok(())
    }

    pub(super) fn capture_all(&self) -> Captured {
        self.capture(false)
    }

    pub(super) fn capture_accepted(&self) -> Captured {
        self.capture(true)
    }

    fn capture(&self, accepted_only: bool) -> Captured {
        #[cfg(feature = "profiling")]
        let _span =
            tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.authority.capture")
                .entered();
        self.reorg.wait_if_pending();
        let state = self.state.read();
        let all_accepted = state.usage.accepted.items == state.owners.len();
        let mut owners = Vec::with_capacity(if accepted_only {
            state.usage.accepted.items
        } else {
            state.owners.len()
        });
        if !accepted_only || all_accepted {
            owners.extend(state.owners.values().cloned());
        } else {
            for entry in state.owners.values() {
                if entry.accepted().is_some() {
                    owners.push(Arc::clone(entry));
                }
            }
        }
        let independent_leaves = state.children.is_empty()
            && state.descendants.is_empty()
            && state.cell_deps.len() <= 64
            && state
                .cell_deps
                .keys()
                .all(|key| !state.spenders.contains_key(key));
        Captured {
            view: state.view,
            snapshot: Arc::clone(&state.snapshot),
            owners,
            accepted_revision: state.accepted_revision,
            accepted_removal_revision: state.accepted_removal_revision,
            independent_leaves,
        }
    }

    pub(super) fn install_pending_ranks(
        &self,
        view: u64,
        accepted_revision: u64,
        generation: u64,
        ranks: RankedPriorities,
    ) {
        self.reorg.wait_if_pending();
        let retired = {
            let mut state = self.state.write();
            (state.view == view
                && state.accepted_revision == accepted_revision
                && state.rank_generation == generation)
                .then(|| std::mem::replace(&mut state.pending_ranks, ranks))
        };
        drop(retired);
    }

    pub(super) fn publish_entry_metrics(&self) {
        if ckb_metrics::handle().is_none() {
            return;
        }
        let summary = self.capture_summary();
        let Some(pending) = summary
            .accepted
            .items
            .checked_sub(summary.gap)
            .and_then(|count| count.checked_sub(summary.proposed))
        else {
            ckb_logger::error!(
                "Inconsistent tx-pool entry metrics: accepted={}, gap={}, proposed={}",
                summary.accepted.items,
                summary.gap,
                summary.proposed,
            );
            return;
        };
        crate::metrics::EntryUsage {
            pending,
            gap: summary.gap,
            proposed: summary.proposed,
        }
        .publish();
    }

    pub(super) fn publish_budget_metrics(&self) {
        if ckb_metrics::handle().is_none() {
            return;
        }
        let (pipeline, remote, history) = {
            let state = self.state.read();
            (
                state.usage.pipeline,
                state.usage.remote,
                state.usage.history,
            )
        };
        crate::metrics::KernelUsage {
            total_entries: pipeline.items,
            total_bytes: pipeline.bytes,
            remote_entries: remote.items,
            remote_bytes: remote.bytes,
            conflict_entries: history.items,
            conflict_bytes: history.bytes,
            active_work: self.budget.active_total(),
        }
        .publish();
    }

    pub(super) fn has_queued_work(&self) -> bool {
        !self.queues.is_empty()
    }

    pub(super) fn queued_len(&self) -> usize {
        self.queues.queued_len()
    }

    pub(super) fn has_verify_work(&self) -> bool {
        self.queues.has_verify_work()
    }

    pub(super) fn pop(
        self: &Arc<Self>,
        stage: WorkStage,
        selection: WorkSelection,
    ) -> Result<Option<Job>, Error> {
        if self.is_stopped() {
            return Ok(None);
        }
        let Some((entry, memory)) = self.queues.pop(stage, selection, &self.budget)? else {
            return Ok(None);
        };
        let state = self.state.read();
        Ok(state
            .owners
            .get(&entry.hash_bytes())
            .is_some_and(|current| Arc::ptr_eq(current, &entry))
            .then(|| Job::new(Arc::clone(self), entry, state.view, memory)))
    }

    pub(super) fn wake_page(&self, cursor: &mut Option<OutPointKey>) -> Option<WakePageSnapshot> {
        let state = self.state.read();
        let key = *cursor
            .as_ref()
            .and_then(|after| state.dirty_wakes.range((Excluded(after), Unbounded)).next())
            .or_else(|| state.dirty_wakes.first())?;
        *cursor = Some(key);
        let wake = state.waiters.get(&key)?.wake.as_ref()?;
        let row = state.waiters.get(&key)?;
        let hashes: Vec<[u8; 32]> = row
            .wake_candidates(wake.pass, wake.after.as_ref())
            .take(WAKE_PAGE)
            .copied()
            .collect();
        let after = hashes.last().copied().or(wake.after);
        let entries = hashes
            .iter()
            .filter_map(|hash| state.owners.get(hash).cloned())
            .collect();
        Some(WakePageSnapshot {
            view: state.view,
            snapshot: Arc::clone(&state.snapshot),
            key,
            pass: wake.pass,
            after,
            entries,
        })
    }

    pub(super) fn expired(
        &self,
        now: Instant,
        accepted_before: u64,
        max: usize,
    ) -> Vec<Arc<Entry>> {
        if max == 0 {
            return Vec::new();
        }
        let state = self.state.read();
        let hashes = state
            .deadlines
            .iter()
            .take_while(|(at, _)| *at <= now)
            .map(|(_, hash)| hash)
            .chain(
                state
                    .accepted_times
                    .iter()
                    .take_while(|(at, _)| *at < accepted_before)
                    .map(|(_, hash)| hash),
            );
        hashes
            .filter_map(|hash| state.owners.get(hash).cloned())
            .take(max)
            .collect()
    }

    #[cfg(test)]
    fn run_before_commit(&self) {
        thread_local! {
            static IN_BEFORE_COMMIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                IN_BEFORE_COMMIT.with(|flag| flag.set(false));
            }
        }
        if IN_BEFORE_COMMIT.with(|flag| flag.replace(true)) {
            return;
        }
        let _reset = Reset;
        let hook = self.before_commit.lock().clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    #[cfg(any(test, feature = "internal"))]
    pub(super) fn verify_observer_task(&self, hash: &Byte32) -> Option<Box<dyn FnOnce() + Send>> {
        let hook = self.verify_observer.lock().clone()?;
        let hash = hash.clone();
        Some(Box::new(move || hook(&hash)))
    }

    #[cfg(any(test, feature = "internal"))]
    pub(super) fn run_verify_observer(&self, hash: &Byte32) {
        let hook = self.verify_observer.lock().clone();
        if let Some(hook) = hook {
            hook(hash);
        }
    }

    #[cfg(test)]
    pub(super) fn run_preview_observer(&self, hash: &Byte32) {
        let hook = self.preview_observer.lock().clone();
        if let Some(hook) = hook {
            hook(hash);
        }
    }

    #[cfg(test)]
    pub(super) fn owner_usage(&self) -> super::budget::OwnerUsage {
        self.state.read().usage.to_owner_usage()
    }

    #[cfg(test)]
    pub(super) fn accepted_usage(&self) -> super::budget::Amount {
        self.state.read().usage.accepted
    }

    #[cfg(test)]
    pub(super) fn peer_banned(&self, peer: PeerIndex) -> bool {
        self.state.read().is_peer_banned(peer, Instant::now())
    }

    #[cfg(test)]
    pub(super) fn spender(
        &self,
        point: &OutPoint,
        premises: &mut Premises,
    ) -> Result<Option<Byte32>, Error> {
        let key = OutPointKey::from_out_point(point);
        let spender = self.state.read().spenders.get(&key).copied();
        premises.observe_spender_key(key, spender)?;
        Ok(spender.map(Byte32::new))
    }
}
