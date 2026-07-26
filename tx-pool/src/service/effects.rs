//! Bounded publication of stable-state tx-pool effects.
//!
//! The journal's innermost `try_apply` section proves static capacity, executes
//! total state Apply and appends its immutable batch. No capacity token crosses
//! an await/lock; a supervised publisher isolates fallible endpoints.

use crate::callback::CallbackEvent;
use crate::component::entry::{TxEntry, TxEntrySnapshot};
use crate::component::pool_map::Status;
use crate::error::Reject;
use crate::network::TxPoolNetworkHandle;
use crate::service::TxPoolService;
use crate::service::TxVerificationResult;
use crate::util::compact_packed;
use ckb_channel::TrySendError;
use ckb_logger::{error, info};
use ckb_network::PeerIndex;
use ckb_types::core::TransactionBuilder;
use ckb_types::core::error::OutPointError;
use ckb_types::packed::Byte32;
use futures_util::FutureExt;
use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

pub(crate) const EFFECT_ENVELOPE_BYTES: usize = 128;
/// Conservative charge for a detached parent hash plus its hash-table slack;
/// publication never retains the source transaction backing allocation.
pub(crate) const UNKNOWN_PARENT_HASH_BYTES: usize = 64;
/// Callback snapshot scalars, view handles and cached hashes beyond tx bytes.
const CALLBACK_SNAPSHOT_OVERHEAD_BYTES: usize = std::mem::size_of::<TxEntrySnapshot>() + 64;
/// Pool-mutation rejects contain fixed-format identities/counters. This bound
/// makes the largest-indivisible batch a checked formula, not a heuristic.
pub(crate) const MAX_POOL_MUTATION_REJECT_BYTES: usize = 256;
/// Cap the non-consensus peer-ban diagnostic included in a commit batch.
pub(crate) const MAX_COMMIT_BAN_REASON_BYTES: usize = 1024;
/// Cap attacker-controlled verifier diagnostics retained for observability.
pub(crate) const MAX_RECENT_REJECT_BYTES: usize = 1024;

fn minimum_serialized_transaction_bytes() -> usize {
    static MINIMUM: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
        TransactionBuilder::default()
            .build()
            .data()
            .serialized_size_in_block()
            .max(1)
    });
    *MINIMUM
}

/// Bound pool-mutation effects from total tx bytes and molecule's minimum tx
/// size; each event may retain callback, relay and bounded reject metadata.
pub(crate) fn max_pool_mutation_effect_bytes(event_transaction_bytes: usize) -> usize {
    let minimum = minimum_serialized_transaction_bytes();
    let max_events = event_transaction_bytes.div_ceil(minimum);
    // Worst case for one removed entry: reject callback (which retains the
    // transaction and reject), relayer settlement, and recent-reject write
    // (which retains the reject a second time).
    let per_event_metadata = EFFECT_ENVELOPE_BYTES
        .saturating_mul(3)
        .saturating_add(CALLBACK_SNAPSHOT_OVERHEAD_BYTES)
        .saturating_add(MAX_POOL_MUTATION_REJECT_BYTES.saturating_mul(2));
    event_transaction_bytes.saturating_add(max_events.saturating_mul(per_event_metadata))
}

/// Static largest admission batch derived from P2's cohort cap and configured
/// pool/block limits, never live population.
pub(crate) fn max_submit_effect_bytes(max_pool_bytes: usize, max_block_bytes: usize) -> usize {
    let max_events = crate::constants::MAX_POOL_MUTATION_CANDIDATES.saturating_add(1);
    let transaction_bytes = max_pool_bytes
        .saturating_add(max_block_bytes)
        .min(max_events.saturating_mul(max_block_bytes));
    let pool_effects = max_pool_mutation_effect_bytes(transaction_bytes);
    let coordinator_effects = max_events
        .saturating_mul(
            EFFECT_ENVELOPE_BYTES
                .saturating_mul(2)
                .saturating_add(MAX_POOL_MUTATION_REJECT_BYTES),
        )
        .saturating_add(EFFECT_ENVELOPE_BYTES.saturating_mul(3))
        .saturating_add(MAX_COMMIT_BAN_REASON_BYTES)
        .saturating_add(MAX_RECENT_REJECT_BYTES);
    pool_effects.saturating_add(coordinator_effects).max(4096)
}

pub(crate) fn bounded_commit_ban_reason(reject: &Reject) -> String {
    let mut reason = format!("reject {reject}");
    if reason.len() > MAX_COMMIT_BAN_REASON_BYTES {
        let mut boundary = MAX_COMMIT_BAN_REASON_BYTES;
        while !reason.is_char_boundary(boundary) {
            boundary -= 1;
        }
        reason.truncate(boundary);
    }
    reason
}

fn bounded_recent_reject(reject: &Reject) -> Reject {
    let rendered = reject.to_string();
    if rendered.len() <= MAX_RECENT_REJECT_BYTES {
        return reject.clone();
    }
    Reject::Malformed(
        "tx-pool".to_string(),
        format!(
            "rejection diagnostic omitted after exceeding {} bytes",
            MAX_RECENT_REJECT_BYTES
        ),
    )
}

/// Convert a rich rejection into the exact bounded payload persisted by the
/// recent-reject database. Doing this before enqueue detaches every packed or
/// verifier-owned allocation and makes the journal byte charge exact.
fn serialized_recent_reject(reject: &Reject) -> String {
    fn serialize(reject: Reject) -> String {
        let public: ckb_jsonrpc_types::PoolTransactionReject = reject.into();
        serde_json::to_string(&public)
            .expect("serializing a string-only pool rejection cannot fail")
    }

    let serialized = serialize(bounded_recent_reject(reject));
    if serialized.len() <= MAX_RECENT_REJECT_BYTES {
        return serialized;
    }
    let fallback = serialize(Reject::Malformed(
        "tx-pool rejection diagnostic omitted".to_string(),
        String::new(),
    ));
    assert!(
        fallback.len() <= MAX_RECENT_REJECT_BYTES,
        "fixed recent-reject fallback exceeds its serialized bound"
    );
    fallback
}

fn bounded_pool_mutation_reject(reject: &Reject) -> bool {
    matches!(
        reject,
        Reject::Full(_)
            | Reject::ExceededMaximumAncestorsCount
            | Reject::Resolve(OutPointError::Dead(_) | OutPointError::InvalidHeader(_))
            | Reject::Expiry(_)
            | Reject::RBFRejected(_)
            | Reject::Invalidated(_)
    ) && reject.to_string().len() <= MAX_POOL_MUTATION_REJECT_BYTES
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectJournalError {
    Closed,
    Full,
    BatchTooLarge { bytes: usize, max_bytes: usize },
    AllocationFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum EffectClass {
    /// Peer-originated work may use only the untrusted portion of the journal.
    Remote,
    /// Local, proposal and bounded maintenance work may borrow unused Remote
    /// capacity and the separately provisioned trusted headroom.
    Trusted,
    /// Chain/admin headroom. When this detailed region is saturated, chain
    /// Apply uses the capacity-independent latest-generation reset register.
    Critical,
}

impl EffectClass {
    const REGION_COUNT: usize = 3;

    fn region(self) -> usize {
        self as usize
    }
}

#[derive(Clone)]
pub(crate) enum TxPoolEffect {
    Callback {
        callbacks: Arc<crate::callback::Callbacks>,
        event: CallbackEvent,
    },
    Relay(TxVerificationResult),
    BanPeer {
        peer: PeerIndex,
        duration: Duration,
        reason: String,
    },
    RecentReject {
        store: Arc<crate::component::recent_reject::RecentReject>,
        tx_hash: Byte32,
        serialized: String,
    },
}

impl TxPoolEffect {
    /// Detach packed identities so an envelope cannot pin a transaction/block
    /// backing allocation that its byte charge does not cover.
    fn into_compact(self) -> Self {
        match self {
            Self::Relay(TxVerificationResult::Ok {
                original_peer,
                tx_hash,
            }) => Self::Relay(TxVerificationResult::Ok {
                original_peer,
                tx_hash: compact_packed(&tx_hash),
            }),
            Self::Relay(TxVerificationResult::Reject { tx_hash }) => {
                Self::Relay(TxVerificationResult::Reject {
                    tx_hash: compact_packed(&tx_hash),
                })
            }
            Self::Relay(TxVerificationResult::GenerationReset) => {
                Self::Relay(TxVerificationResult::GenerationReset)
            }
            Self::Relay(TxVerificationResult::UnknownParents { peer, parents }) => {
                Self::Relay(TxVerificationResult::UnknownParents {
                    peer,
                    parents: parents
                        .into_iter()
                        .map(|parent| compact_packed(&parent))
                        .collect(),
                })
            }
            Self::RecentReject {
                store,
                tx_hash,
                serialized,
            } => Self::RecentReject {
                store,
                tx_hash: compact_packed(&tx_hash),
                serialized,
            },
            effect @ (Self::Callback { .. } | Self::BanPeer { .. }) => effect,
        }
    }

    pub(crate) fn charge_bytes(&self) -> usize {
        match self {
            Self::Callback {
                event: CallbackEvent::Pending(entry),
                ..
            }
            | Self::Callback {
                event: CallbackEvent::Proposed(entry),
                ..
            } => EFFECT_ENVELOPE_BYTES.saturating_add(entry.charge_bytes()),
            Self::Callback {
                event: CallbackEvent::Reject(entry, reject),
                ..
            } => EFFECT_ENVELOPE_BYTES
                .saturating_add(entry.charge_bytes())
                .saturating_add(reject.to_string().len()),
            Self::Relay(TxVerificationResult::UnknownParents { parents, .. }) => {
                EFFECT_ENVELOPE_BYTES
                    .saturating_add(parents.len().saturating_mul(UNKNOWN_PARENT_HASH_BYTES))
            }
            Self::Relay(
                TxVerificationResult::Ok { .. }
                | TxVerificationResult::Reject { .. }
                | TxVerificationResult::GenerationReset,
            ) => EFFECT_ENVELOPE_BYTES,
            Self::BanPeer { reason, .. } => EFFECT_ENVELOPE_BYTES.saturating_add(reason.len()),
            Self::RecentReject { serialized, .. } => {
                EFFECT_ENVELOPE_BYTES.saturating_add(serialized.len())
            }
        }
    }
}

pub(crate) struct EffectBatch {
    effects: Vec<TxPoolEffect>,
    next: AtomicUsize,
    charge_bytes: usize,
}

impl EffectBatch {
    pub(crate) fn new(effects: Vec<TxPoolEffect>) -> Option<Self> {
        if effects.is_empty() {
            return None;
        }
        let effects: Vec<_> = effects
            .into_iter()
            .map(TxPoolEffect::into_compact)
            .collect();
        let charge_bytes = effects.iter().fold(0usize, |total, effect| {
            total.saturating_add(effect.charge_bytes())
        });
        Some(Self {
            effects,
            next: AtomicUsize::new(0),
            charge_bytes,
        })
    }

    fn reset_record() -> Self {
        Self {
            effects: vec![TxPoolEffect::Relay(TxVerificationResult::GenerationReset)],
            next: AtomicUsize::new(0),
            charge_bytes: 0,
        }
    }

    pub(crate) fn charge_bytes(&self) -> usize {
        self.charge_bytes
    }

    fn current(&self) -> Option<&TxPoolEffect> {
        self.effects.get(self.next.load(Ordering::Acquire))
    }

    fn advance(&self) {
        self.next.fetch_add(1, Ordering::AcqRel);
    }

    fn is_complete(&self) -> bool {
        self.next.load(Ordering::Acquire) >= self.effects.len()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EffectUsage {
    pub(crate) batches: usize,
    pub(crate) bytes: usize,
}

impl EffectUsage {
    const fn new(batches: usize, bytes: usize) -> Self {
        Self { batches, bytes }
    }

    fn fits(self, bytes: usize, limit: Self) -> bool {
        self.batches
            .checked_add(1)
            .is_some_and(|value| value <= limit.batches)
            && self
                .bytes
                .checked_add(bytes)
                .is_some_and(|value| value <= limit.bytes)
    }

    fn charge(&mut self, bytes: usize) {
        self.batches += 1;
        self.bytes += bytes;
    }

    fn release(&mut self, bytes: usize) -> bool {
        let Some(batches) = self.batches.checked_sub(1) else {
            return false;
        };
        let Some(total_bytes) = self.bytes.checked_sub(bytes) else {
            return false;
        };
        self.batches = batches;
        self.bytes = total_bytes;
        true
    }
}

struct EffectEnvelope {
    sequence: u128,
    /// `None` is the statically owned latest-generation reset slot.
    class: Option<EffectClass>,
    batch: Arc<EffectBatch>,
}

struct JournalState {
    queued: VecDeque<EffectEnvelope>,
    active: Option<EffectEnvelope>,
    latest_generation_reset: Option<EffectEnvelope>,
    /// Cumulative region lattice: Remote batches charge all three slots,
    /// Trusted batches charge ordinary+total, and Critical charges total.
    usage: [EffectUsage; EffectClass::REGION_COUNT],
    next_sequence: u128,
    closed: bool,
}

impl JournalState {
    fn charge(&mut self, class: EffectClass, bytes: usize) {
        for usage in &mut self.usage[class.region()..] {
            usage.charge(bytes);
        }
    }

    fn release(&mut self, class: EffectClass, bytes: usize) -> bool {
        self.usage[class.region()..]
            .iter_mut()
            .all(|usage| usage.release(bytes))
    }

    fn allocate_sequence(&mut self) -> u128 {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("u128 effect sequence exhausted");
        sequence
    }

    fn push_generation_reset(&mut self, reset_batch: &Arc<EffectBatch>) {
        let sequence = self.allocate_sequence();
        self.latest_generation_reset = Some(EffectEnvelope {
            sequence,
            class: None,
            batch: Arc::clone(reset_batch),
        });
    }

    fn push(&mut self, class: EffectClass, batch: EffectBatch) {
        let sequence = self.allocate_sequence();
        self.charge(class, batch.charge_bytes);
        self.queued.push_back(EffectEnvelope {
            sequence,
            class: Some(class),
            batch: Arc::new(batch),
        });
    }

    /// Rebuild accounting from the two authoritative resident containers.
    /// This is cold defect recovery only; normal publication is O(1).
    fn recompute_usage(&mut self) {
        let mut usage = [EffectUsage::default(); EffectClass::REGION_COUNT];
        for envelope in self.active.iter().chain(self.queued.iter()) {
            if let Some(class) = envelope.class {
                for region in &mut usage[class.region()..] {
                    region.charge(envelope.batch.charge_bytes);
                }
            }
        }
        self.usage = usage;
    }
}

struct CallbackJob {
    callbacks: Arc<crate::callback::Callbacks>,
    event: CallbackEvent,
    done: tokio::sync::oneshot::Sender<()>,
}

/// Stable, statically partitioned effect journal.
///
/// Callers construct immutable batches before mutation. The innermost journal
/// lock checks capacity, executes total Apply and appends before opening;
/// Full therefore never changes state.
pub(crate) struct EffectJournal {
    state: Mutex<JournalState>,
    ready: Notify,
    space: Notify,
    limits: [EffectUsage; EffectClass::REGION_COUNT],
    publisher_running: AtomicBool,
    callback_circuit_open: AtomicBool,
    callback_sender: std::sync::mpsc::SyncSender<CallbackJob>,
    /// Prebuilt, allocation-free emergency authority record. Installing a
    /// reset under a chain/defect lock only clones this Arc into the one
    /// replaceable slot.
    generation_reset_batch: Arc<EffectBatch>,
}

impl EffectJournal {
    pub(crate) fn new_partitioned(
        remote_max_batches: usize,
        remote_max_bytes: usize,
        trusted_headroom_batches: usize,
        trusted_headroom_bytes: usize,
        critical_batches: usize,
        critical_bytes: usize,
    ) -> Result<Self, EffectJournalError> {
        let ordinary_batches = remote_max_batches
            .checked_add(trusted_headroom_batches)
            .ok_or(EffectJournalError::AllocationFailed)?;
        let ordinary_bytes = remote_max_bytes
            .checked_add(trusted_headroom_bytes)
            .ok_or(EffectJournalError::AllocationFailed)?;
        let total_batches = ordinary_batches
            .checked_add(critical_batches)
            .ok_or(EffectJournalError::AllocationFailed)?;
        let total_bytes = ordinary_bytes
            .checked_add(critical_bytes)
            .ok_or(EffectJournalError::AllocationFailed)?;
        if remote_max_batches == 0 || remote_max_bytes == 0 {
            return Err(EffectJournalError::AllocationFailed);
        }
        let mut queued = VecDeque::new();
        queued
            .try_reserve(total_batches)
            .map_err(|_| EffectJournalError::AllocationFailed)?;
        let (callback_sender, callback_receiver) = std::sync::mpsc::sync_channel::<CallbackJob>(1);
        std::thread::Builder::new()
            .name("tx-pool-callback".to_owned())
            .spawn(move || {
                while let Ok(job) = callback_receiver.recv() {
                    job.callbacks.publish(&job.event);
                    let _ = job.done.send(());
                }
            })
            .map_err(|_| EffectJournalError::AllocationFailed)?;
        Ok(Self {
            state: Mutex::new(JournalState {
                queued,
                active: None,
                latest_generation_reset: None,
                usage: [EffectUsage::default(); EffectClass::REGION_COUNT],
                next_sequence: 1,
                closed: false,
            }),
            ready: Notify::new(),
            space: Notify::new(),
            limits: [
                EffectUsage::new(remote_max_batches, remote_max_bytes),
                EffectUsage::new(ordinary_batches, ordinary_bytes),
                EffectUsage::new(total_batches, total_bytes),
            ],
            publisher_running: AtomicBool::new(false),
            callback_circuit_open: AtomicBool::new(false),
            callback_sender,
            generation_reset_batch: Arc::new(EffectBatch::reset_record()),
        })
    }

    fn class_limit(&self, class: EffectClass) -> EffectUsage {
        self.limits[class.region()]
    }

    fn fits(&self, state: &JournalState, class: EffectClass, bytes: usize) -> bool {
        state.usage[class.region()..]
            .iter()
            .zip(&self.limits[class.region()..])
            .all(|(usage, limit)| usage.fits(bytes, *limit))
    }

    /// Install a post-Apply batch only when it honors the bound checked before
    /// mutation. A violated proof cannot be returned to the caller after state
    /// changed, so converge observers through the capacity-independent reset
    /// register instead of overcharging the FIFO.
    fn push_bounded_or_reset(
        &self,
        state: &mut JournalState,
        class: EffectClass,
        proven_bytes: usize,
        batch: EffectBatch,
    ) {
        let actual = batch.charge_bytes;
        if actual <= proven_bytes && self.fits(state, class, actual) {
            state.push(class, batch);
        } else {
            error!(
                "tx-pool effect plan violated its bound: actual {actual}, proven {proven_bytes}; publishing generation reset"
            );
            state.push_generation_reset(&self.generation_reset_batch);
        }
    }

    /// Wait only for a level-triggered capacity *hint*. This does not reserve
    /// anything. The caller must re-plan and call `try_apply` under authority
    /// locks; a racing append may legitimately return `Full` again.
    pub(crate) async fn wait_capacity(
        &self,
        bytes: usize,
        class: EffectClass,
    ) -> Result<(), EffectJournalError> {
        let max_bytes = self.class_limit(class).bytes;
        if bytes > max_bytes {
            return Err(EffectJournalError::BatchTooLarge { bytes, max_bytes });
        }
        loop {
            let space = self.space.notified();
            tokio::pin!(space);
            space.as_mut().enable();
            {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.closed {
                    return Err(EffectJournalError::Closed);
                }
                if self.fits(&state, class, bytes) {
                    return Ok(());
                }
            }
            space.await;
        }
    }

    /// Atomically validate exact journal capacity, execute a total state Apply
    /// and append its immutable effect batch. `apply` is never called on
    /// `Closed`, `Full`, or oversized input.
    pub(crate) fn try_apply<T>(
        &self,
        batch: Option<EffectBatch>,
        class: EffectClass,
        apply: impl FnOnce() -> T,
    ) -> Result<T, EffectJournalError> {
        let Some(batch) = batch else {
            return Ok(apply());
        };
        let bytes = batch.charge_bytes;
        let max_bytes = self.class_limit(class).bytes;
        if bytes > max_bytes {
            return Err(EffectJournalError::BatchTooLarge { bytes, max_bytes });
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return Err(EffectJournalError::Closed);
        }
        if !self.fits(&state, class, bytes) {
            return Err(EffectJournalError::Full);
        }
        let result = apply();
        state.push(class, batch);
        drop(state);
        self.ready.notify_one();
        Ok(result)
    }

    /// Transitional form for effects materialized from an immutable pool plan
    /// during total Apply; its static upper bound is checked before Apply.
    pub(crate) fn try_apply_bounded<T>(
        &self,
        max_bytes: usize,
        class: EffectClass,
        apply: impl FnOnce() -> (T, Option<EffectBatch>),
    ) -> Result<T, EffectJournalError> {
        let class_max = self.class_limit(class).bytes;
        if max_bytes > class_max {
            return Err(EffectJournalError::BatchTooLarge {
                bytes: max_bytes,
                max_bytes: class_max,
            });
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return Err(EffectJournalError::Closed);
        }
        if !self.fits(&state, class, max_bytes) {
            return Err(EffectJournalError::Full);
        }
        let (result, batch) = apply();
        if let Some(batch) = batch {
            self.push_bounded_or_reset(&mut state, class, max_bytes, batch);
        }
        drop(state);
        self.ready.notify_one();
        Ok(result)
    }

    /// Apply a chain-authoritative transition without waiting behind detailed
    /// publication. If the critical FIFO can hold the proven batch, `apply`
    /// receives `true` and its detail is appended normally. Otherwise it
    /// receives `false`, the state transition still linearizes, and one
    /// prebuilt replaceable GenerationReset is installed instead.
    ///
    /// This is intentionally narrower than `try_apply_bounded`: ordinary
    /// admission may backpressure, while chain convergence may discard
    /// observational detail but cannot wait behind callbacks or relay retry.
    pub(crate) fn try_apply_authoritative<T>(
        &self,
        max_detail_bytes: usize,
        apply: impl FnOnce(bool) -> (T, Option<EffectBatch>),
    ) -> Result<T, EffectJournalError> {
        let class = EffectClass::Critical;
        let class_max = self.class_limit(class).bytes;
        if max_detail_bytes > class_max {
            return Err(EffectJournalError::BatchTooLarge {
                bytes: max_detail_bytes,
                max_bytes: class_max,
            });
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return Err(EffectJournalError::Closed);
        }
        let publish_detail = self.fits(&state, class, max_detail_bytes);
        let (result, batch) = apply(publish_detail);
        if publish_detail {
            if let Some(batch) = batch {
                self.push_bounded_or_reset(&mut state, class, max_detail_bytes, batch);
            }
        } else {
            debug_assert!(batch.is_none(), "reset fallback must not retain detail");
            state.push_generation_reset(&self.generation_reset_batch);
        }
        drop(state);
        self.ready.notify_one();
        Ok(result)
    }

    pub(crate) async fn append(
        &self,
        batch: EffectBatch,
        class: EffectClass,
    ) -> Result<(), EffectJournalError> {
        let bytes = batch.charge_bytes;
        let max_bytes = self.class_limit(class).bytes;
        if bytes > max_bytes {
            return Err(EffectJournalError::BatchTooLarge { bytes, max_bytes });
        }
        let mut batch = Some(batch);
        loop {
            self.wait_capacity(bytes, class).await?;
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return Err(EffectJournalError::Closed);
            }
            if !self.fits(&state, class, bytes) {
                continue;
            }
            state.push(class, batch.take().expect("batch appended once"));
            drop(state);
            self.ready.notify_one();
            return Ok(());
        }
    }

    /// Install the one replaceable, statically resident relayer reset record.
    /// It participates in the journal sequence but never waits for or borrows
    /// FIFO capacity. Repeated resets coalesce to the latest authority.
    pub(crate) fn install_generation_reset(&self) -> Result<(), EffectJournalError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return Err(EffectJournalError::Closed);
        }
        state.push_generation_reset(&self.generation_reset_batch);
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    pub(crate) fn close(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.closed = true;
        }
        // There is exactly one publisher. `notify_one` stores a permit when
        // close races with its idle check, unlike `notify_waiters`.
        self.ready.notify_one();
        self.space.notify_waiters();
    }
}

#[derive(Clone)]
pub(crate) struct EffectEndpoints {
    pub(crate) network: TxPoolNetworkHandle,
    pub(crate) tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
}

struct PublisherClaim<'a>(&'a AtomicBool);

impl Drop for PublisherClaim<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl EffectJournal {
    fn checkout(&self) -> Option<(u128, Arc<EffectBatch>)> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.active.is_none() {
            let queued_sequence = state.queued.front().map(|envelope| envelope.sequence);
            let reset_sequence = state
                .latest_generation_reset
                .as_ref()
                .map(|envelope| envelope.sequence);
            state.active = match (queued_sequence, reset_sequence) {
                (Some(queued), Some(reset)) if reset < queued => {
                    state.latest_generation_reset.take()
                }
                (None, Some(_)) => state.latest_generation_reset.take(),
                _ => state.queued.pop_front(),
            };
        }
        state
            .active
            .as_ref()
            .map(|envelope| (envelope.sequence, Arc::clone(&envelope.batch)))
    }

    fn complete(&self, sequence: u128) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(active) = state.active.as_ref() else {
            error!("effect publisher completion had no active batch");
            return;
        };
        if active.sequence != sequence {
            error!(
                "effect publisher completion sequence mismatch: expected {}, got {}",
                active.sequence, sequence
            );
            return;
        }
        let bytes = active.batch.charge_bytes;
        let class = active.class;
        state.active.take();
        if let Some(class) = class
            && !state.release(class, bytes)
        {
            error!("effect journal accounting drift detected; rebuilding from resident batches");
            state.recompute_usage();
            drop(state);
            self.space.notify_waiters();
            return;
        }
        drop(state);
        self.space.notify_waiters();
    }

    fn closed_and_empty(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed
            && state.active.is_none()
            && state.queued.is_empty()
            && state.latest_generation_reset.is_none()
    }
}

#[cfg(not(test))]
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(test)]
const CALLBACK_TIMEOUT: Duration = Duration::from_millis(50);
const RELAY_RETRY_TIMEOUT: Duration = Duration::from_millis(250);

async fn publish_callback(
    queue: &EffectJournal,
    callbacks: Arc<crate::callback::Callbacks>,
    event: CallbackEvent,
) {
    if queue.callback_circuit_open.load(Ordering::Acquire) {
        error!("tx-pool callback circuit is open; dropping callback notification");
        return;
    }
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    if let Err(error) = queue.callback_sender.try_send(CallbackJob {
        callbacks,
        event,
        done: done_tx,
    }) {
        queue.callback_circuit_open.store(true, Ordering::Release);
        error!("tx-pool callback endpoint unavailable: {error}");
        return;
    }
    if !matches!(
        tokio::time::timeout(CALLBACK_TIMEOUT, done_rx).await,
        Ok(Ok(()))
    ) {
        queue.callback_circuit_open.store(true, Ordering::Release);
        error!("tx-pool callback timed out; callback circuit opened");
    }
}

async fn publish_one(queue: &EffectJournal, endpoints: &EffectEndpoints, effect: TxPoolEffect) {
    match effect {
        TxPoolEffect::Callback { callbacks, event } => {
            publish_callback(queue, callbacks, event).await;
        }
        TxPoolEffect::Relay(result) => {
            let started = tokio::time::Instant::now();
            let mut pending = result;
            loop {
                match endpoints.tx_relay_sender.try_send(pending) {
                    Ok(()) => break,
                    Err(TrySendError::Full(returned))
                        if started.elapsed() < RELAY_RETRY_TIMEOUT =>
                    {
                        pending = returned;
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    Err(TrySendError::Full(_)) => {
                        error!("tx-pool relayer endpoint remained full; dropping bounded effect");
                        break;
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        error!("tx-pool relayer result receiver dropped");
                        break;
                    }
                }
            }
        }
        TxPoolEffect::BanPeer {
            peer,
            duration,
            reason,
        } => endpoints.network.ban_peer(peer, duration, reason),
        TxPoolEffect::RecentReject {
            store,
            tx_hash,
            serialized,
        } => {
            if let Err(error) = store.put_serialized(&tx_hash, &serialized) {
                error!("failed to record recent reject {}: {}", tx_hash, error);
            }
        }
    }
}

async fn run_effect_publisher_once(queue: Arc<EffectJournal>, endpoints: EffectEndpoints) {
    if queue
        .publisher_running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        error!("refusing concurrent tx-pool effect publisher");
        return;
    }
    let _claim = PublisherClaim(&queue.publisher_running);
    loop {
        let ready = queue.ready.notified();
        let Some((sequence, batch)) = queue.checkout() else {
            if queue.closed_and_empty() {
                return;
            }
            ready.await;
            continue;
        };
        while let Some(effect) = batch.current().cloned() {
            match AssertUnwindSafe(publish_one(&queue, &endpoints, effect))
                .catch_unwind()
                .await
            {
                Ok(()) => batch.advance(),
                Err(payload) => {
                    error!(
                        "tx-pool effect endpoint panicked and was quarantined: {}",
                        crate::util::panic_payload_to_string(payload.as_ref())
                    );
                    batch.advance();
                }
            }
        }
        if batch.is_complete() {
            queue.complete(sequence);
        }
    }
}

/// Supervise the sole publisher. An unwind releases only the publisher claim;
/// the active batch remains charged in the stable journal and is resumed from
/// its per-effect cursor by the replacement loop.
pub(crate) async fn run_effect_publisher(queue: Arc<EffectJournal>, endpoints: EffectEndpoints) {
    loop {
        let result = AssertUnwindSafe(run_effect_publisher_once(
            Arc::clone(&queue),
            endpoints.clone(),
        ))
        .catch_unwind()
        .await;
        match result {
            Ok(()) if queue.closed_and_empty() => break,
            Ok(()) => tokio::task::yield_now().await,
            Err(payload) => {
                error!(
                    "restarting tx-pool effect publisher after panic: {}",
                    crate::util::panic_payload_to_string(payload.as_ref())
                );
                tokio::task::yield_now().await;
            }
        }
    }
    info!("tx-pool effect publisher drained and exited");
}

pub(crate) fn callback_accept(
    callbacks: Arc<crate::callback::Callbacks>,
    entry: TxEntry,
    status: Status,
) -> TxPoolEffect {
    let event = match status {
        Status::Proposed => CallbackEvent::Proposed(entry.into()),
        Status::Pending | Status::Gap => CallbackEvent::Pending(entry.into()),
    };
    TxPoolEffect::Callback { callbacks, event }
}

pub(crate) fn callback_reject(
    callbacks: Arc<crate::callback::Callbacks>,
    entry: TxEntry,
    reject: Reject,
) -> TxPoolEffect {
    TxPoolEffect::Callback {
        callbacks,
        event: CallbackEvent::Reject(entry.into(), reject),
    }
}

impl TxPoolService {
    pub(crate) fn unknown_parents_effect_bytes(parent_count: usize) -> usize {
        EFFECT_ENVELOPE_BYTES.saturating_add(parent_count.saturating_mul(UNKNOWN_PARENT_HASH_BYTES))
    }

    pub(crate) fn max_submit_effect_bytes(&self) -> usize {
        max_submit_effect_bytes(
            self.pool.tx_pool_config.max_tx_pool_size,
            self.pool.consensus.max_block_bytes() as usize,
        )
    }

    pub(crate) fn max_reorg_effect_bytes(&self) -> usize {
        // Reorg notifications are coalesced to one final full-hash event per
        // still-resident entry, and terminal entries emit reject instead of
        // an intermediate notification. No new entry is inserted inside the
        // locked reorg mutation, so total referenced tx bytes are at most P.
        max_pool_mutation_effect_bytes(self.pool.tx_pool_config.max_tx_pool_size).max(4096)
    }

    pub(crate) async fn publish_effects(&self, effects: Vec<TxPoolEffect>) {
        self.publish_effects_class(effects, EffectClass::Trusted)
            .await;
    }

    pub(crate) async fn publish_effects_class(
        &self,
        effects: Vec<TxPoolEffect>,
        class: EffectClass,
    ) {
        let Some(batch) = EffectBatch::new(effects) else {
            return;
        };
        if let Err(error) = self.relay.effects.append(batch, class).await {
            // Closure means service shutdown. Oversize is a construction or
            // configuration defect, but it must not turn attacker input into
            // a process-wide fail-stop.
            error!("tx-pool standalone effect was not journaled: {error:?}");
        }
    }

    pub(crate) fn accepted_effect(&self, entry: TxEntry, status: Status) -> Option<TxPoolEffect> {
        let registered = match status {
            Status::Proposed => self.relay.callbacks.proposed.is_some(),
            Status::Pending | Status::Gap => self.relay.callbacks.pending.is_some(),
        };
        registered.then(|| callback_accept(Arc::clone(&self.relay.callbacks), entry, status))
    }

    /// Complete terminal pool-removal publication. Keep relayer and recent
    /// reject semantics in the tx-pool core instead of hiding them inside a
    /// user callback, so every deployment observes the same outcome.
    pub(crate) fn rejected_effects(&self, entry: TxEntry, reject: Reject) -> Vec<TxPoolEffect> {
        assert!(
            bounded_pool_mutation_reject(&reject),
            "unbounded reject escaped a submit/reorg pool mutation: {reject}"
        );
        let mut effects = Vec::new();
        if let Some(effect) = self.recent_reject_effect(entry.transaction().hash(), &reject) {
            effects.push(effect);
        }
        if self.relay.callbacks.reject.is_some() {
            effects.push(callback_reject(
                Arc::clone(&self.relay.callbacks),
                entry.clone(),
                reject.clone(),
            ));
        }
        if reject.is_allowed_relay() && !matches!(reject, Reject::Duplicated(_)) {
            effects.push(TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: entry.transaction().hash(),
            }));
        }
        effects
    }

    pub(crate) fn recent_reject_effect(
        &self,
        tx_hash: Byte32,
        reject: &Reject,
    ) -> Option<TxPoolEffect> {
        if !reject.should_recorded() {
            return None;
        }
        self.aux
            .recent_reject
            .as_ref()
            .map(|store| TxPoolEffect::RecentReject {
                store: Arc::clone(store),
                tx_hash,
                serialized: serialized_recent_reject(reject),
            })
    }

    pub(crate) async fn publish_relay_result(&self, result: TxVerificationResult) {
        self.publish_effects(vec![TxPoolEffect::Relay(result)])
            .await;
    }
}

#[cfg(test)]
#[path = "tests/effects.rs"]
mod tests;
