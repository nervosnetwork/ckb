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
use ckb_util::{Mutex, MutexGuard};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
/// Conservative residency charge for the journal's hash-to-envelope
/// projection used by immediate RPC rejection reads.
const PENDING_REJECT_INDEX_BYTES: usize = 128;

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
        .saturating_add(PENDING_REJECT_INDEX_BYTES)
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
        .saturating_add(PENDING_REJECT_INDEX_BYTES)
        .saturating_add(MAX_RECENT_REJECT_BYTES);
    pool_effects.saturating_add(coordinator_effects).max(4096)
}

pub(crate) fn bounded_commit_ban_reason(reject: &Reject) -> String {
    let mut reason = format!("reject {reject}");
    if reason.len() > MAX_COMMIT_BAN_REASON_BYTES {
        let boundary = reason.floor_char_boundary(MAX_COMMIT_BAN_REASON_BYTES);
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
#[derive(Debug)]
enum RecentRejectEncodingError {
    Json(serde_json::Error),
    FixedFallbackExceedsBound,
}

impl std::fmt::Display for RecentRejectEncodingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "recent-reject JSON encoding failed: {error}"),
            Self::FixedFallbackExceedsBound => {
                write!(formatter, "fixed recent-reject fallback exceeds its bound")
            }
        }
    }
}

fn serialized_recent_reject(reject: &Reject) -> Result<String, RecentRejectEncodingError> {
    fn serialize(reject: Reject) -> Result<String, RecentRejectEncodingError> {
        let public: ckb_jsonrpc_types::PoolTransactionReject = reject.into();
        serde_json::to_string(&public).map_err(RecentRejectEncodingError::Json)
    }

    let serialized = serialize(bounded_recent_reject(reject))?;
    if serialized.len() <= MAX_RECENT_REJECT_BYTES {
        return Ok(serialized);
    }
    let fallback = serialize(Reject::Malformed(
        "tx-pool rejection diagnostic omitted".to_string(),
        String::new(),
    ))?;
    if fallback.len() > MAX_RECENT_REJECT_BYTES {
        return Err(RecentRejectEncodingError::FixedFallbackExceedsBound);
    }
    Ok(fallback)
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
    SequenceExhausted,
    Projection(&'static str),
}

/// Closed error domain for the capacity wait operation.
///
/// Waiting consumes `Full` internally and performs no journal mutation, so it
/// cannot produce sequence/projection/allocation failures. Exposing those
/// variants here previously let callers collapse a violated startup size
/// proof into ordinary shutdown with `.is_err()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffectCapacityWaitError {
    Closed,
    BatchTooLarge { bytes: usize, max_bytes: usize },
}

impl From<EffectCapacityWaitError> for EffectJournalError {
    fn from(error: EffectCapacityWaitError) -> Self {
        match error {
            EffectCapacityWaitError::Closed => Self::Closed,
            EffectCapacityWaitError::BatchTooLarge { bytes, max_bytes } => {
                Self::BatchTooLarge { bytes, max_bytes }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffectBuildError {
    UnboundedPoolMutationReject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
            Self::RecentReject { serialized, .. } => EFFECT_ENVELOPE_BYTES
                .saturating_add(PENDING_REJECT_INDEX_BYTES)
                .saturating_add(serialized.len()),
        }
    }
}

pub(crate) struct EffectBatch {
    effects: Vec<TxPoolEffect>,
    recent_rejects: Box<[(Byte32, usize)]>,
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
        let recent_rejects = effects
            .iter()
            .enumerate()
            .filter_map(|(index, effect)| {
                if let TxPoolEffect::RecentReject { tx_hash, .. } = effect {
                    Some((tx_hash.clone(), index))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Some(Self {
            effects,
            recent_rejects,
            next: AtomicUsize::new(0),
            charge_bytes,
        })
    }

    fn reset_record() -> Self {
        Self {
            effects: vec![TxPoolEffect::Relay(TxVerificationResult::GenerationReset)],
            recent_rejects: Box::new([]),
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

    fn advance(&self) -> Result<(), EffectJournalError> {
        self.next
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < self.effects.len()).then(|| current.saturating_add(1))
            })
            .map(|_| ())
            .map_err(|_| EffectJournalError::Projection("effect cursor advance"))
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

    fn checked_charge(self, bytes: usize) -> Option<Self> {
        Some(Self {
            batches: self.batches.checked_add(1)?,
            bytes: self.bytes.checked_add(bytes)?,
        })
    }

    fn checked_release(self, bytes: usize) -> Option<Self> {
        Some(Self {
            batches: self.batches.checked_sub(1)?,
            bytes: self.bytes.checked_sub(bytes)?,
        })
    }
}

struct EffectEnvelope {
    sequence: u128,
    /// `None` is the statically owned latest-generation reset slot.
    class: Option<EffectClass>,
    batch: Arc<EffectBatch>,
}

struct PendingReject {
    sequence: u128,
    batch: Arc<EffectBatch>,
    effect_index: usize,
}

struct EffectAppendPlan {
    sequence: u128,
    usage: EffectRegions,
}

/// Cumulative capacity lattice for the three ingress trust classes.
///
/// Named fields encode the containment relation directly and avoid using a
/// trust-class discriminant as an unchecked slice boundary.
#[derive(Clone, Copy, Debug, Default)]
struct EffectRegions {
    remote: EffectUsage,
    ordinary: EffectUsage,
    total: EffectUsage,
}

impl EffectRegions {
    fn new(remote: EffectUsage, ordinary: EffectUsage, total: EffectUsage) -> Self {
        Self {
            remote,
            ordinary,
            total,
        }
    }

    fn checked_charge(self, class: EffectClass, bytes: usize) -> Option<Self> {
        match class {
            EffectClass::Remote => Some(Self {
                remote: self.remote.checked_charge(bytes)?,
                ordinary: self.ordinary.checked_charge(bytes)?,
                total: self.total.checked_charge(bytes)?,
            }),
            EffectClass::Trusted => Some(Self {
                remote: self.remote,
                ordinary: self.ordinary.checked_charge(bytes)?,
                total: self.total.checked_charge(bytes)?,
            }),
            EffectClass::Critical => Some(Self {
                remote: self.remote,
                ordinary: self.ordinary,
                total: self.total.checked_charge(bytes)?,
            }),
        }
    }

    fn checked_release(self, class: EffectClass, bytes: usize) -> Option<Self> {
        match class {
            EffectClass::Remote => Some(Self {
                remote: self.remote.checked_release(bytes)?,
                ordinary: self.ordinary.checked_release(bytes)?,
                total: self.total.checked_release(bytes)?,
            }),
            EffectClass::Trusted => Some(Self {
                remote: self.remote,
                ordinary: self.ordinary.checked_release(bytes)?,
                total: self.total.checked_release(bytes)?,
            }),
            EffectClass::Critical => Some(Self {
                remote: self.remote,
                ordinary: self.ordinary,
                total: self.total.checked_release(bytes)?,
            }),
        }
    }

    fn limit_for(self, class: EffectClass) -> EffectUsage {
        match class {
            EffectClass::Remote => self.remote,
            EffectClass::Trusted => self.ordinary,
            EffectClass::Critical => self.total,
        }
    }

    fn fits(self, limits: Self, class: EffectClass, bytes: usize) -> bool {
        match class {
            EffectClass::Remote => {
                self.remote.fits(bytes, limits.remote)
                    && self.ordinary.fits(bytes, limits.ordinary)
                    && self.total.fits(bytes, limits.total)
            }
            EffectClass::Trusted => {
                self.ordinary.fits(bytes, limits.ordinary) && self.total.fits(bytes, limits.total)
            }
            EffectClass::Critical => self.total.fits(bytes, limits.total),
        }
    }
}

struct JournalState {
    queued: VecDeque<EffectEnvelope>,
    active: Option<EffectEnvelope>,
    latest_generation_reset: Option<EffectEnvelope>,
    /// Read-only projection of already charged journal records. It closes the
    /// interval between accepted Apply and recent-reject persistence without
    /// performing I/O under a transaction authority lock.
    pending_recent_rejects: HashMap<Byte32, PendingReject>,
    /// Cumulative region lattice: Remote batches charge all three slots,
    /// Trusted batches charge ordinary+total, and Critical charges total.
    usage: EffectRegions,
    next_sequence: u128,
    closed: bool,
}

impl JournalState {
    fn release(&mut self, class: EffectClass, bytes: usize) -> Result<(), EffectJournalError> {
        self.usage =
            self.usage
                .checked_release(class, bytes)
                .ok_or(EffectJournalError::Projection(
                    "active batch exceeds journal accounting",
                ))?;
        Ok(())
    }

    fn reserve_sequence(&mut self) -> Result<u128, EffectJournalError> {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(EffectJournalError::SequenceExhausted)?;
        Ok(sequence)
    }

    fn apply_generation_reset(&mut self, sequence: u128, reset_batch: &Arc<EffectBatch>) {
        self.latest_generation_reset = Some(EffectEnvelope {
            sequence,
            class: None,
            batch: Arc::clone(reset_batch),
        });
    }

    fn plan_append(
        &mut self,
        class: EffectClass,
        bytes: usize,
    ) -> Result<EffectAppendPlan, EffectJournalError> {
        let sequence = self.reserve_sequence()?;
        self.plan_append_with_sequence(class, bytes, sequence)
    }

    fn plan_append_with_sequence(
        &self,
        class: EffectClass,
        bytes: usize,
        sequence: u128,
    ) -> Result<EffectAppendPlan, EffectJournalError> {
        let usage =
            self.usage
                .checked_charge(class, bytes)
                .ok_or(EffectJournalError::Projection(
                    "effect append accounting overflow",
                ))?;
        Ok(EffectAppendPlan { sequence, usage })
    }

    fn apply_append(&mut self, plan: EffectAppendPlan, class: EffectClass, batch: EffectBatch) {
        self.usage = plan.usage;
        let batch = Arc::new(batch);
        for (tx_hash, effect_index) in batch.recent_rejects.iter() {
            self.pending_recent_rejects.insert(
                tx_hash.clone(),
                PendingReject {
                    sequence: plan.sequence,
                    batch: Arc::clone(&batch),
                    effect_index: *effect_index,
                },
            );
        }
        self.queued.push_back(EffectEnvelope {
            sequence: plan.sequence,
            class: Some(class),
            batch,
        });
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
    limits: EffectRegions,
    publisher_running: AtomicBool,
    callback_circuit_open: AtomicBool,
    network_circuit_open: AtomicBool,
    recent_reject_circuit_open: AtomicBool,
    callback_sender: std::sync::mpsc::SyncSender<CallbackJob>,
    /// Prebuilt, allocation-free emergency authority record. Installing a
    /// reset under a chain/defect lock only clones this Arc into the one
    /// replaceable slot.
    generation_reset_batch: Arc<EffectBatch>,
}

/// Publication paired with one chain-authoritative state transition.
/// Construction is private; callers receive an [`AuthoritativeCapacity`]
/// capability and cannot return detail when only the reset slot was reserved.
enum AuthoritativePublication {
    Detail(Option<EffectBatch>),
    GenerationReset,
}

pub(crate) struct AuthoritativeCommit<T> {
    result: T,
    publication: AuthoritativePublication,
}

/// Exact publication capacity reserved while chain authority and the journal
/// are held. The capability converts detail into a reset automatically when
/// the critical FIFO lacks room, making the old `(false, Detail)` mismatch
/// unrepresentable at the API boundary.
#[derive(Clone, Copy)]
pub(crate) struct AuthoritativeCapacity {
    retains_detail: bool,
}

impl AuthoritativeCapacity {
    pub(crate) fn retains_detail(self) -> bool {
        self.retains_detail
    }

    pub(crate) fn detail<T>(self, result: T, batch: Option<EffectBatch>) -> AuthoritativeCommit<T> {
        let publication = if self.retains_detail {
            AuthoritativePublication::Detail(batch)
        } else {
            AuthoritativePublication::GenerationReset
        };
        AuthoritativeCommit {
            result,
            publication,
        }
    }

    pub(crate) fn reset<T>(self, result: T) -> AuthoritativeCommit<T> {
        AuthoritativeCommit {
            result,
            publication: AuthoritativePublication::GenerationReset,
        }
    }
}

impl EffectJournal {
    fn lock_state(&self) -> MutexGuard<'_, JournalState> {
        self.state.lock()
    }

    pub(crate) fn new_partitioned(
        remote_max_batches: usize,
        remote_max_bytes: usize,
        trusted_headroom_batches: usize,
        trusted_headroom_bytes: usize,
        critical_batches: usize,
        critical_bytes: usize,
    ) -> Result<Self, EffectJournalError> {
        // The service builder derives these region sizes from the largest
        // indivisible submit/reorg formulas. This constructor owns allocation,
        // overflow and the minimum usable Remote-region checks; it cannot
        // re-derive workload bounds from six already-materialized capacities.
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
                crate::callback::mark_callback_thread();
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
                pending_recent_rejects: HashMap::new(),
                usage: EffectRegions::default(),
                next_sequence: 1,
                closed: false,
            }),
            ready: Notify::new(),
            space: Notify::new(),
            limits: EffectRegions::new(
                EffectUsage::new(remote_max_batches, remote_max_bytes),
                EffectUsage::new(ordinary_batches, ordinary_bytes),
                EffectUsage::new(total_batches, total_bytes),
            ),
            publisher_running: AtomicBool::new(false),
            callback_circuit_open: AtomicBool::new(false),
            network_circuit_open: AtomicBool::new(false),
            recent_reject_circuit_open: AtomicBool::new(false),
            callback_sender,
            generation_reset_batch: Arc::new(EffectBatch::reset_record()),
        })
    }

    fn class_limit(&self, class: EffectClass) -> EffectUsage {
        self.limits.limit_for(class)
    }

    fn fits(&self, state: &JournalState, class: EffectClass, bytes: usize) -> bool {
        state.usage.fits(self.limits, class, bytes)
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
        sequence: u128,
        batch: EffectBatch,
    ) {
        let actual = batch.charge_bytes;
        if actual <= proven_bytes && self.fits(state, class, actual) {
            match state.plan_append_with_sequence(class, actual, sequence) {
                Ok(plan) => state.apply_append(plan, class, batch),
                Err(error) => {
                    error!(
                        "tx-pool effect append plan failed after authoritative Apply: {error:?}; publishing generation reset"
                    );
                    state.apply_generation_reset(sequence, &self.generation_reset_batch);
                }
            }
        } else {
            error!(
                "tx-pool effect plan violated its bound: actual {actual}, proven {proven_bytes}; publishing generation reset"
            );
            state.apply_generation_reset(sequence, &self.generation_reset_batch);
        }
    }

    /// Wait only for a level-triggered capacity *hint*. This does not reserve
    /// anything. The caller must re-plan and call `try_apply` under authority
    /// locks; a racing append may legitimately return `Full` again.
    pub(crate) async fn wait_capacity(
        &self,
        bytes: usize,
        class: EffectClass,
    ) -> Result<(), EffectCapacityWaitError> {
        let max_bytes = self.class_limit(class).bytes;
        if bytes > max_bytes {
            return Err(EffectCapacityWaitError::BatchTooLarge { bytes, max_bytes });
        }
        loop {
            let space = self.space.notified();
            tokio::pin!(space);
            space.as_mut().enable();
            {
                let state = self.lock_state();
                if state.closed {
                    return Err(EffectCapacityWaitError::Closed);
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
        let mut state = self.lock_state();
        if state.closed {
            return Err(EffectJournalError::Closed);
        }
        if !self.fits(&state, class, bytes) {
            return Err(EffectJournalError::Full);
        }
        let plan = state.plan_append(class, bytes)?;
        let result = apply();
        state.apply_append(plan, class, batch);
        drop(state);
        self.ready.notify_one();
        Ok(result)
    }

    /// Variant for an exclusive prepared mutation whose sole fallible step is
    /// guaranteed to precede every physical state change. A failed preflight
    /// leaves authority unchanged and therefore must not publish its effects.
    pub(crate) fn try_apply_checked<T, E>(
        &self,
        batch: Option<EffectBatch>,
        class: EffectClass,
        apply: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, EffectJournalError> {
        let Some(batch) = batch else {
            return Ok(apply());
        };
        let bytes = batch.charge_bytes;
        let max_bytes = self.class_limit(class).bytes;
        if bytes > max_bytes {
            return Err(EffectJournalError::BatchTooLarge { bytes, max_bytes });
        }
        let mut state = self.lock_state();
        if state.closed {
            return Err(EffectJournalError::Closed);
        }
        if !self.fits(&state, class, bytes) {
            return Err(EffectJournalError::Full);
        }
        let plan = state.plan_append(class, bytes)?;
        match apply() {
            Ok(result) => {
                state.apply_append(plan, class, batch);
                drop(state);
                self.ready.notify_one();
                Ok(Ok(result))
            }
            Err(error) => Ok(Err(error)),
        }
    }

    /// Apply a chain-authoritative transition without waiting behind detailed
    /// publication. If the critical FIFO can hold the proven batch, `apply`
    /// receives `true` and its detail is appended normally. Otherwise it
    /// receives `false`, the state transition still linearizes, and one
    /// prebuilt replaceable GenerationReset is installed instead.
    ///
    /// Ordinary admission uses exact prebuilt batches and may backpressure;
    /// only chain convergence may discard observational detail rather than
    /// wait behind callbacks or relay retry.
    pub(crate) fn try_apply_authoritative<T>(
        &self,
        max_detail_bytes: usize,
        apply: impl FnOnce(AuthoritativeCapacity) -> AuthoritativeCommit<T>,
    ) -> Result<T, EffectJournalError> {
        let class = EffectClass::Critical;
        let class_max = self.class_limit(class).bytes;
        if max_detail_bytes > class_max {
            return Err(EffectJournalError::BatchTooLarge {
                bytes: max_detail_bytes,
                max_bytes: class_max,
            });
        }
        let mut state = self.lock_state();
        if state.closed {
            return Err(EffectJournalError::Closed);
        }
        let sequence = state.reserve_sequence()?;
        let capacity = AuthoritativeCapacity {
            retains_detail: self.fits(&state, class, max_detail_bytes),
        };
        let AuthoritativeCommit {
            result,
            publication,
        } = apply(capacity);
        match publication {
            AuthoritativePublication::GenerationReset => {
                state.apply_generation_reset(sequence, &self.generation_reset_batch);
            }
            AuthoritativePublication::Detail(batch) => {
                if let Some(batch) = batch {
                    self.push_bounded_or_reset(
                        &mut state,
                        class,
                        max_detail_bytes,
                        sequence,
                        batch,
                    );
                }
            }
        }
        drop(state);
        self.ready.notify_one();
        Ok(result)
    }

    /// Linearize an authority generation swap with its replaceable reset
    /// publication. The state closure never runs when the journal is closed
    /// or its sequence is exhausted, so callers retain the complete old
    /// generation instead of committing an externally invisible reset.
    pub(crate) fn apply_generation_reset<T>(
        &self,
        apply: impl FnOnce() -> T,
    ) -> Result<T, EffectJournalError> {
        self.try_apply_authoritative(0, |capacity| capacity.reset(apply()))
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
            self.wait_capacity(bytes, class)
                .await
                .map_err(EffectJournalError::from)?;
            let mut state = self.lock_state();
            if state.closed {
                return Err(EffectJournalError::Closed);
            }
            if !self.fits(&state, class, bytes) {
                continue;
            }
            let plan = state.plan_append(class, bytes)?;
            let Some(batch) = batch.take() else {
                return Err(EffectJournalError::Projection(
                    "effect batch appended twice",
                ));
            };
            state.apply_append(plan, class, batch);
            drop(state);
            self.ready.notify_one();
            return Ok(());
        }
    }

    /// Install the one replaceable, statically resident relayer reset record.
    /// It participates in the journal sequence but never waits for or borrows
    /// FIFO capacity. Repeated resets coalesce to the latest authority.
    pub(crate) fn install_generation_reset(&self) -> Result<(), EffectJournalError> {
        let mut state = self.lock_state();
        if state.closed {
            return Err(EffectJournalError::Closed);
        }
        let sequence = state.reserve_sequence()?;
        state.apply_generation_reset(sequence, &self.generation_reset_batch);
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    pub(crate) fn close(&self) {
        {
            let mut state = self.lock_state();
            state.closed = true;
        }
        // There is exactly one publisher. `notify_one` stores a permit when
        // close races with its idle check, unlike `notify_waiters`.
        self.ready.notify_one();
        self.space.notify_waiters();
    }

    /// Return a rejection committed to the charged journal but not necessarily
    /// persisted yet. The index points into the immutable batch and is removed
    /// only after that batch finishes publication.
    pub(crate) fn pending_recent_reject(&self, hash: &Byte32) -> Option<String> {
        let state = self.lock_state();
        let pending = state.pending_recent_rejects.get(hash)?;
        match pending.batch.effects.get(pending.effect_index)? {
            TxPoolEffect::RecentReject {
                tx_hash,
                serialized,
                ..
            } if tx_hash == hash => Some(serialized.clone()),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct EffectEndpoints {
    pub(crate) network: TxPoolNetworkHandle,
    pub(crate) tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
}

struct PublisherClaim<'a>(&'a AtomicBool);

struct CheckedOutBatch {
    sequence: u128,
    batch: Arc<EffectBatch>,
}

impl Drop for PublisherClaim<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl EffectJournal {
    fn checkout(&self) -> Option<CheckedOutBatch> {
        let mut state = self.lock_state();
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
        state.active.as_ref().map(|envelope| CheckedOutBatch {
            sequence: envelope.sequence,
            batch: Arc::clone(&envelope.batch),
        })
    }

    fn complete(&self, checked_out: CheckedOutBatch) -> Result<(), EffectJournalError> {
        let mut state = self.lock_state();
        let active = state.active.as_ref().ok_or(EffectJournalError::Projection(
            "effect completion lacks active batch",
        ))?;
        if active.sequence != checked_out.sequence
            || !Arc::ptr_eq(&active.batch, &checked_out.batch)
        {
            return Err(EffectJournalError::Projection(
                "effect completion capability does not match active batch",
            ));
        }
        let bytes = active.batch.charge_bytes;
        let class = active.class;
        let batch = Arc::clone(&active.batch);
        // Accounting is the only fallible completion step. Settle it before
        // deleting any read projection so an error leaves the active envelope
        // and all observer-visible metadata intact.
        if let Some(class) = class {
            state.release(class, bytes)?;
        }
        for (tx_hash, _) in batch.recent_rejects.iter() {
            if state
                .pending_recent_rejects
                .get(tx_hash)
                .is_some_and(|pending| pending.sequence == checked_out.sequence)
            {
                state.pending_recent_rejects.remove(tx_hash);
            }
        }
        state.active.take();
        drop(state);
        self.space.notify_waiters();
        Ok(())
    }

    fn closed_and_empty(&self) -> bool {
        let state = self.lock_state();
        state.closed
            && state.active.is_none()
            && state.queued.is_empty()
            && state.latest_generation_reset.is_none()
    }

    fn is_closed(&self) -> bool {
        self.lock_state().closed
    }
}

/// Maximum time the ordered publisher grants one foreign endpoint.
///
/// Authority has already committed before these calls run. A callback,
/// injected network handle or auxiliary database must therefore be allowed
/// to lose only its own observational detail, never retain the journal head
/// indefinitely and turn a foreign stall into admission backpressure.
const EXTERNAL_EFFECT_TIMEOUT: Duration = Duration::from_secs(1);
const RELAY_RETRY_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug)]
enum BlockingEffectFailure {
    TimedOut,
    Task(tokio::task::JoinError),
}

impl std::fmt::Display for BlockingEffectFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut => formatter.write_str("timed out"),
            Self::Task(error) => write!(formatter, "task failed: {error}"),
        }
    }
}

/// Execute one potentially blocking foreign endpoint behind the common
/// publication deadline. Tokio cannot cancel a blocking closure that has
/// started, but the caller opens the endpoint's circuit on error, so at most
/// one detached invocation per endpoint kind can remain stuck.
async fn run_blocking_effect<T: Send + 'static>(
    operation: impl FnOnce() -> T + Send + 'static,
) -> Result<T, BlockingEffectFailure> {
    match tokio::time::timeout(
        EXTERNAL_EFFECT_TIMEOUT,
        tokio::task::spawn_blocking(operation),
    )
    .await
    {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => Err(BlockingEffectFailure::Task(error)),
        Err(_) => Err(BlockingEffectFailure::TimedOut),
    }
}

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
        tokio::time::timeout(EXTERNAL_EFFECT_TIMEOUT, done_rx).await,
        Ok(Ok(()))
    ) {
        queue.callback_circuit_open.store(true, Ordering::Release);
        error!("tx-pool callback timed out; callback circuit opened");
    }
}

async fn publish_ban_peer(
    queue: &EffectJournal,
    network: TxPoolNetworkHandle,
    peer: PeerIndex,
    duration: Duration,
    reason: String,
) {
    if queue.network_circuit_open.load(Ordering::Acquire) {
        error!("tx-pool network effect circuit is open; dropping peer ban");
        return;
    }
    // TxPoolNetwork is an injected, potentially foreign endpoint. A Tokio
    // task boundary turns its unwind into a typed JoinError without making
    // catch_unwind part of journal or authority control flow.
    if let Err(error) = run_blocking_effect(move || {
        network.ban_peer(peer, duration, reason);
    })
    .await
    {
        queue.network_circuit_open.store(true, Ordering::Release);
        error!("tx-pool network effect task failed; circuit opened: {error}");
    }
}

async fn publish_recent_reject(
    queue: &EffectJournal,
    store: Arc<crate::component::recent_reject::RecentReject>,
    tx_hash: Byte32,
    serialized: String,
) {
    if queue.recent_reject_circuit_open.load(Ordering::Acquire) {
        error!("tx-pool recent-reject circuit is open; dropping persistence effect");
        return;
    }
    let published = run_blocking_effect(move || {
        store
            .put_serialized(&tx_hash, &serialized)
            .map_err(|error| format!("failed to record recent reject {tx_hash}: {error}"))
    })
    .await;
    match published {
        Ok(Ok(())) => {}
        Ok(Err(error)) => error!("{error}"),
        Err(error) => {
            queue
                .recent_reject_circuit_open
                .store(true, Ordering::Release);
            error!("tx-pool recent-reject effect task failed; circuit opened: {error}");
        }
    }
}

async fn publish_one(queue: &EffectJournal, endpoints: &EffectEndpoints, effect: TxPoolEffect) {
    match effect {
        TxPoolEffect::Callback { callbacks, event } => {
            publish_callback(queue, callbacks, event).await;
        }
        TxPoolEffect::Relay(result) => {
            let is_reset = matches!(result, TxVerificationResult::GenerationReset);
            let is_parent_request = matches!(result, TxVerificationResult::UnknownParents { .. });
            let started = tokio::time::Instant::now();
            let mut pending = result;
            loop {
                match endpoints.tx_relay_sender.try_send(pending) {
                    Ok(()) => break,
                    Err(TrySendError::Full(returned)) => {
                        if queue.is_closed() {
                            break;
                        }
                        if !is_reset
                            && !is_parent_request
                            && started.elapsed() >= RELAY_RETRY_TIMEOUT
                        {
                            // Individual results are no longer reliable once
                            // the internal sink has stayed saturated. Replace
                            // them with the existing constant-size authority:
                            // clearing the relayer's known set is conservative
                            // and makes every dropped terminal edge recoverable.
                            if let Err(error) = queue.install_generation_reset()
                                && error != EffectJournalError::Closed
                            {
                                error!("relayer reconciliation install failed: {error:?}");
                                queue.close();
                            }
                            error!(
                                "tx-pool relayer endpoint remained full; coalescing to GenerationReset"
                            );
                            break;
                        }
                        pending = returned;
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        error!("tx-pool relayer result receiver dropped");
                        // The receiver is a required consumer for essential
                        // parent requests. Close the journal so supervision
                        // quiesces the generation instead of draining and
                        // silently discarding every later committed effect.
                        queue.close();
                        break;
                    }
                }
            }
        }
        TxPoolEffect::BanPeer {
            peer,
            duration,
            reason,
        } => {
            publish_ban_peer(
                queue,
                Arc::clone(&endpoints.network),
                peer,
                duration,
                reason,
            )
            .await;
        }
        TxPoolEffect::RecentReject {
            store,
            tx_hash,
            serialized,
        } => publish_recent_reject(queue, store, tx_hash, serialized).await,
    }
}

async fn run_effect_publisher_once(
    queue: Arc<EffectJournal>,
    endpoints: EffectEndpoints,
) -> Result<(), EffectJournalError> {
    if queue
        .publisher_running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        error!("refusing concurrent tx-pool effect publisher");
        return Err(EffectJournalError::Projection(
            "concurrent effect publisher",
        ));
    }
    let _claim = PublisherClaim(&queue.publisher_running);
    loop {
        let ready = queue.ready.notified();
        let Some(checked_out) = queue.checkout() else {
            if queue.closed_and_empty() {
                return Ok(());
            }
            ready.await;
            continue;
        };
        while let Some(effect) = checked_out.batch.current().cloned() {
            publish_one(&queue, &endpoints, effect).await;
            checked_out.batch.advance()?;
        }
        if checked_out.batch.is_complete() {
            queue.complete(checked_out)?;
        }
    }
}

pub(crate) async fn run_effect_publisher(queue: Arc<EffectJournal>, endpoints: EffectEndpoints) {
    if let Err(error) = run_effect_publisher_once(Arc::clone(&queue), endpoints).await {
        error!("tx-pool effect publisher stopped on journal fault: {error:?}");
        queue.close();
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
    pub(crate) fn rejected_effects(
        &self,
        entry: TxEntry,
        reject: Reject,
    ) -> Result<Vec<TxPoolEffect>, EffectBuildError> {
        if !bounded_pool_mutation_reject(&reject) {
            return Err(EffectBuildError::UnboundedPoolMutationReject);
        }
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
        Ok(effects)
    }

    pub(crate) fn recent_reject_effect(
        &self,
        tx_hash: Byte32,
        reject: &Reject,
    ) -> Option<TxPoolEffect> {
        if !reject.should_recorded() {
            return None;
        }
        let store = self.aux.recent_reject.as_ref()?;
        let serialized = match serialized_recent_reject(reject) {
            Ok(serialized) => serialized,
            Err(error) => {
                error!("failed to encode bounded recent reject: {error}");
                return None;
            }
        };
        Some(TxPoolEffect::RecentReject {
            store: Arc::clone(store),
            tx_hash,
            serialized,
        })
    }

    pub(crate) async fn publish_relay_result(&self, result: TxVerificationResult) {
        self.publish_effects(vec![TxPoolEffect::Relay(result)])
            .await;
    }

    /// Publish duplicate acceptance only while the accepted authority still
    /// proves membership. Holding the pool read guard through journal append
    /// orders this observation against reorg/clear: either `Ok` precedes the
    /// later removal/reset, or the transaction is absent and no stale success
    /// can be published after that reset.
    pub(crate) async fn publish_accepted_relay_result(
        &self,
        tx_hash: Byte32,
        original_peer: Option<PeerIndex>,
    ) -> Result<bool, EffectJournalError> {
        let class = if original_peer.is_some() {
            EffectClass::Remote
        } else {
            EffectClass::Trusted
        };
        loop {
            self.relay
                .effects
                .wait_capacity(EFFECT_ENVELOPE_BYTES, class)
                .await?;
            let tx_pool = self.pool.tx_pool.read().await;
            if tx_pool.get_tx_from_pool_by_hash(&tx_hash).is_none() {
                return Ok(false);
            }
            let batch = EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Ok {
                original_peer,
                tx_hash: tx_hash.clone(),
            })])
            .ok_or(EffectJournalError::Projection(
                "accepted relay effect is empty",
            ))?;
            match self.relay.effects.try_apply(Some(batch), class, || ()) {
                Ok(()) => return Ok(true),
                Err(EffectJournalError::Full) => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/effects.rs"]
mod tests;
