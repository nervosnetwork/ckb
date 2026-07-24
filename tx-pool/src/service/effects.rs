//! Bounded publication of stable-state tx-pool effects.
//!
//! State owners enqueue immutable batches after their authoritative mutation.
//! A single publisher, independent of the controller dispatcher, preserves
//! batch order and contains callback re-entry. The outbox charges queued and
//! active payloads continuously; a full outbox applies backpressure without
//! retaining an unbounded task/channel backlog.

use crate::callback::CallbackEvent;
#[cfg(test)]
use crate::component::effect_outbox::EffectOutboxUsage;
use crate::component::effect_outbox::{EffectOutbox, EffectOutboxError, EffectOutboxLimits};
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
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub(crate) const EFFECT_ENVELOPE_BYTES: usize = 128;
/// Conservative allocator-independent charge for one detached parent hash in
/// an `UnknownParents` `HashSet`. The packed value itself is only part of the
/// cost: the set also owns bucket/control slack. Parent hashes are detached
/// from their source transaction before publication, so this charge never
/// hides retention of an entire packed transaction backing allocation.
pub(crate) const UNKNOWN_PARENT_HASH_BYTES: usize = 64;
/// Fixed callback snapshot residency not represented by the serialized
/// transaction itself: scalar accounting fields, view handles, and the two
/// cached transaction hashes.
const CALLBACK_SNAPSHOT_OVERHEAD_BYTES: usize = std::mem::size_of::<TxEntrySnapshot>() + 64;
/// Pool-mutation reject reasons are generated from fixed-format hashes,
/// outpoints, counters, and fee rates. Keeping the display bound explicit
/// turns submit/reorg reservations into a checked formula rather than a
/// heuristic multiplier. An unexpected variant or longer description is a
/// fail-stop invariant violation before the effect is journaled.
pub(crate) const MAX_POOL_MUTATION_REJECT_BYTES: usize = 256;
/// A malformed commit may append one peer-ban diagnostic to the same batch as
/// the pool mutation. Diagnostics are not consensus data; cap this one path so
/// an attacker-controlled error display cannot invalidate a pre-mutation
/// reservation after the authoritative state has changed.
pub(crate) const MAX_COMMIT_BAN_REASON_BYTES: usize = 1024;
/// Rejections outside the tightly enumerated pool-mutation family may carry
/// verifier diagnostics. Recent-reject persistence is observability, not
/// consensus state, so cap the retained diagnostic instead of allowing an
/// attacker-controlled string to dominate outbox residency.
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

/// Maximum outbox charge for stable pool-mutation effects whose transaction
/// payloads total at most `event_transaction_bytes`. Every event retains one
/// transaction, at most one callback envelope, at most one relay envelope,
/// and one bounded reject description. The molecule minimum transaction size
/// converts the serialized-byte bound into a conservative event-count bound.
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

/// Complete reservation for one authoritative submit mutation.
///
/// Pool callbacks/relays are bounded by `P + B`. A coordinator commit can also
/// settle at most every currently resident pre-pool owner: each contributes at
/// most one relay envelope, and the failed-winner path can add one extra ban
/// envelope plus its bounded diagnostic. Keeping both terms explicit makes the
/// bound independent of the relative pool/coordinator configuration sizes.
pub(crate) fn max_submit_effect_bytes(
    max_pool_bytes: usize,
    max_block_bytes: usize,
    max_pipeline_records: usize,
) -> usize {
    let pool_effects =
        max_pool_mutation_effect_bytes(max_pool_bytes.saturating_add(max_block_bytes));
    // Every conflict loser can produce one relay plus one bounded
    // recent-reject write. The failed winner can additionally produce one
    // bounded ban, relay and larger verifier rejection.
    let coordinator_effects = max_pipeline_records
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
/// verifier-owned allocation and makes the outbox byte charge exact.
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
pub(crate) enum EffectQueueError {
    Closed,
    BatchTooLarge { bytes: usize, max_bytes: usize },
    Invariant(EffectOutboxError),
}

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
    /// Detach every small packed identity that may outlive its producer. This
    /// is the final stable-effect residency boundary, so callers cannot
    /// accidentally enqueue a 32-byte slice that pins a transaction or block
    /// backing allocation while the outbox charges only the envelope.
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
            Self::Relay(TxVerificationResult::Ok { .. } | TxVerificationResult::Reject { .. }) => {
                EFFECT_ENVELOPE_BYTES
            }
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

struct QueueState {
    outbox: EffectOutbox<Arc<EffectBatch>>,
    closed: bool,
}

/// Shared bounded journal. All methods that mutate the core are synchronous;
/// the only await in `enqueue` happens while waiting for pre-mutation capacity.
pub(crate) struct EffectQueue {
    state: Mutex<QueueState>,
    ready: Notify,
    space: Notify,
    quiescent: Notify,
    ordinary_max_batches: usize,
    ordinary_max_bytes: usize,
    max_bytes: usize,
}

pub(crate) struct EffectPermit {
    queue: Arc<EffectQueue>,
    reservation: Option<crate::component::effect_outbox::EffectReservation>,
    bytes: usize,
}

impl Drop for EffectPermit {
    fn drop(&mut self) {
        let Some(reservation) = self.reservation.take() else {
            return;
        };
        let cancelled = {
            let mut state = self.queue.state.lock().unwrap_or_else(|e| e.into_inner());
            state.outbox.cancel(&reservation)
        };
        if let Err(error) = cancelled {
            error!("failed to cancel unused effect reservation: {:?}", error);
        }
        self.queue.space.notify_waiters();
    }
}

impl EffectPermit {
    /// Commit to the queue that issued this permit. The permit owns both the
    /// reservation and queue identity, so callers cannot bind a valid credit
    /// to a different outbox. Sequence allocation and enqueue are one atomic
    /// operation at the authoritative mutation boundary.
    pub(crate) fn commit(mut self, batch: EffectBatch) -> Result<(), EffectQueueError> {
        if batch.charge_bytes > self.bytes {
            return Err(EffectQueueError::BatchTooLarge {
                bytes: batch.charge_bytes,
                max_bytes: self.bytes,
            });
        }
        let reservation = self
            .reservation
            .as_ref()
            .ok_or(EffectQueueError::Invariant(
                EffectOutboxError::MissingReservation,
            ))?;
        let queue = Arc::clone(&self.queue);
        let batch = Arc::new(batch);
        let result = {
            let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .outbox
                .commit_reserved(reservation, batch.charge_bytes, batch)
        };
        if let Err(error) = result {
            queue.space.notify_waiters();
            return Err(EffectQueueError::Invariant(error));
        }
        // The reservation was consumed by `commit_reserved`; disarm Drop.
        self.reservation.take();
        // A conservative permit may have been shrunk substantially above.
        // Wake capacity waiters now; waiting for this batch to publish can
        // deadlock when its head is a causal barrier released by one of those
        // waiters.
        queue.space.notify_waiters();
        queue.ready.notify_one();
        Ok(())
    }
}

impl EffectQueue {
    #[cfg(test)]
    pub(crate) fn new(max_batches: usize, max_bytes: usize) -> Result<Self, EffectOutboxError> {
        Self::new_with_critical_capacity(max_batches, max_bytes, 0, 0)
    }

    pub(crate) fn new_with_critical_capacity(
        ordinary_max_batches: usize,
        ordinary_max_bytes: usize,
        critical_batches: usize,
        critical_bytes: usize,
    ) -> Result<Self, EffectOutboxError> {
        let max_batches = ordinary_max_batches
            .checked_add(critical_batches)
            .ok_or(EffectOutboxError::AllocationFailed)?;
        let max_bytes = ordinary_max_bytes
            .checked_add(critical_bytes)
            .ok_or(EffectOutboxError::AllocationFailed)?;
        Ok(Self {
            state: Mutex::new(QueueState {
                outbox: EffectOutbox::new(EffectOutboxLimits::new(max_batches, max_bytes))?,
                closed: false,
            }),
            ready: Notify::new(),
            space: Notify::new(),
            quiescent: Notify::new(),
            ordinary_max_batches,
            ordinary_max_bytes,
            max_bytes,
        })
    }

    pub(crate) async fn reserve(
        self: &Arc<Self>,
        bytes: usize,
    ) -> Result<EffectPermit, EffectQueueError> {
        self.reserve_inner(bytes, false).await
    }

    /// Reserve from the capacity kept outside the ordinary traffic budget.
    /// Critical batches still share the same FIFO sequence/outbox; the extra
    /// headroom prevents ordinary relay/callback backpressure from starving a
    /// chain-state transition before it begins.
    pub(crate) async fn reserve_critical(
        self: &Arc<Self>,
        bytes: usize,
    ) -> Result<EffectPermit, EffectQueueError> {
        self.reserve_inner(bytes, true).await
    }

    async fn reserve_inner(
        self: &Arc<Self>,
        bytes: usize,
        critical: bool,
    ) -> Result<EffectPermit, EffectQueueError> {
        let class_max_bytes = if critical {
            self.max_bytes
        } else {
            self.ordinary_max_bytes
        };
        if bytes > class_max_bytes {
            return Err(EffectQueueError::BatchTooLarge {
                bytes,
                max_bytes: class_max_bytes,
            });
        }
        loop {
            let space = self.space.notified();
            enum Attempt<T> {
                Reserved(T),
                Wait,
                Error(EffectOutboxError),
            }
            let attempt = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.closed {
                    return Err(EffectQueueError::Closed);
                }
                let usage = state.outbox.usage();
                if !critical
                    && (usage.batches.saturating_add(1) > self.ordinary_max_batches
                        || usage.bytes.saturating_add(bytes) > self.ordinary_max_bytes)
                {
                    Attempt::Wait
                } else {
                    match state.outbox.reserve(bytes) {
                        Ok(reservation) => Attempt::Reserved(reservation),
                        Err(EffectOutboxError::BatchLimitExceeded)
                        | Err(EffectOutboxError::ByteLimitExceeded) => Attempt::Wait,
                        Err(error) => Attempt::Error(error),
                    }
                }
            };
            match attempt {
                Attempt::Reserved(reservation) => {
                    return Ok(EffectPermit {
                        queue: Arc::clone(self),
                        reservation: Some(reservation),
                        bytes,
                    });
                }
                Attempt::Wait => space.await,
                Attempt::Error(error) => return Err(EffectQueueError::Invariant(error)),
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn enqueue(
        self: &Arc<Self>,
        batch: EffectBatch,
    ) -> Result<(), EffectQueueError> {
        let permit = self.reserve(batch.charge_bytes).await?;
        permit.commit(batch)
    }

    pub(crate) fn close(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.closed = true;
        }
        self.ready.notify_waiters();
        self.space.notify_waiters();
    }

    #[cfg(test)]
    pub(crate) fn usage(&self) -> EffectOutboxUsage {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .outbox
            .usage()
    }

    #[cfg(test)]
    pub(crate) async fn wait_idle(&self) {
        loop {
            let quiescent = self.quiescent.notified();
            if self.usage().batches == 0 {
                return;
            }
            quiescent.await;
        }
    }
}

#[derive(Clone)]
pub(crate) struct EffectEndpoints {
    pub(crate) network: TxPoolNetworkHandle,
    pub(crate) tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
    /// Fatal journal invariants cancel the complete tx-pool service. Merely
    /// logging and exiting would leave producers blocked behind a publisher
    /// that can no longer consume the bounded outbox.
    pub(crate) failure_cancel: CancellationToken,
}

enum PublishOne {
    Complete,
    Retry,
}

impl EffectEndpoints {
    fn publish_one(&self, effect: &TxPoolEffect) -> PublishOne {
        match effect {
            TxPoolEffect::Callback { callbacks, event } => callbacks.publish(event),
            TxPoolEffect::Relay(result) => match self.tx_relay_sender.try_send(result.clone()) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => return PublishOne::Retry,
                Err(TrySendError::Disconnected(_)) => {
                    error!("tx-pool relayer result receiver dropped")
                }
            },
            TxPoolEffect::BanPeer {
                peer,
                duration,
                reason,
            } => self.network.ban_peer(*peer, *duration, reason.clone()),
            TxPoolEffect::RecentReject {
                store,
                tx_hash,
                serialized,
            } => {
                // `RecentReject::put` owns the RocksDB blocking boundary; do
                // not nest another `block_in_place` around it here.
                if let Err(error) = store.put_serialized(tx_hash, serialized) {
                    error!("failed to record recent reject {}: {}", tx_hash, error);
                }
            }
        }
        PublishOne::Complete
    }
}

/// Drain until `close` has been called and every queued/active batch is done.
pub(crate) async fn run_effect_publisher(queue: Arc<EffectQueue>, endpoints: EffectEndpoints) {
    enum Checkout {
        Idle {
            closed: bool,
            empty: bool,
        },
        Batch {
            sequence: u64,
            batch: Arc<EffectBatch>,
        },
        Fatal(EffectOutboxError),
    }

    let fail = |error: EffectOutboxError| -> ! {
        queue.close();
        endpoints.failure_cancel.cancel();
        panic!("tx-pool effect outbox invariant failure: {error:?}");
    };

    loop {
        let ready = queue.ready.notified();
        let checkout = {
            let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
            let closed = state.closed;
            match state.outbox.checkout() {
                Ok(Some(sequence)) => match state.outbox.active_effect(sequence) {
                    Ok(batch) => Checkout::Batch {
                        sequence,
                        batch: Arc::clone(batch),
                    },
                    Err(error) => Checkout::Fatal(error),
                },
                Ok(None) => Checkout::Idle {
                    closed,
                    empty: state.outbox.usage().batches == 0,
                },
                Err(error) => Checkout::Fatal(error),
            }
        };

        let (sequence, batch) = match checkout {
            Checkout::Idle { closed, empty } => {
                if closed && empty {
                    break;
                }
                queue.quiescent.notify_waiters();
                ready.await;
                continue;
            }
            Checkout::Batch { sequence, batch } => (sequence, batch),
            Checkout::Fatal(error) => fail(error),
        };

        while let Some(effect) = batch.current() {
            match catch_unwind(AssertUnwindSafe(|| endpoints.publish_one(effect))) {
                Ok(PublishOne::Complete) => batch.advance(),
                Ok(PublishOne::Retry) => {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                Err(payload) => {
                    error!(
                        "tx-pool effect publisher contained endpoint panic: {}",
                        crate::util::panic_payload_to_string(payload.as_ref())
                    );
                    // An endpoint can have performed an arbitrary prefix of
                    // its side effect before unwinding. Retrying it is neither
                    // at-most-once nor safe, and a permanent infrastructure
                    // panic must not pin every later stable-state effect
                    // behind this FIFO head. Callback panics are already
                    // contained inside `Callbacks::publish`; this guard is the
                    // final quarantine boundary for unexpected network or
                    // publisher endpoint failures.
                    batch.advance();
                }
            }
        }
        if batch.is_complete() {
            let completed = {
                let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
                state.outbox.complete_active(sequence)
            };
            if let Err(error) = completed {
                fail(error);
            }
            queue.space.notify_waiters();
            queue.quiescent.notify_waiters();
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
        // Successful submit effects reference a disjoint subset of the
        // pre-commit pool plus the accepted transaction. Failed submissions
        // restore old entries and suppress their transient reject events.
        // Therefore their total transaction bytes are bounded by P + B.
        max_submit_effect_bytes(
            self.pool.tx_pool_config.max_tx_pool_size,
            self.pool.consensus.max_block_bytes() as usize,
            self.pipeline.runtime.max_entries(),
        )
    }

    pub(crate) fn max_reorg_effect_bytes(&self) -> usize {
        // Reorg notifications are coalesced to one final full-hash event per
        // still-resident entry, and terminal entries emit reject instead of
        // an intermediate notification. No new entry is inserted inside the
        // locked reorg mutation, so total referenced tx bytes are at most P.
        max_pool_mutation_effect_bytes(self.pool.tx_pool_config.max_tx_pool_size).max(4096)
    }

    pub(crate) async fn reserve_effects(
        &self,
        bytes: usize,
    ) -> Result<EffectPermit, EffectQueueError> {
        self.relay.effects.reserve(bytes).await
    }

    /// Required stable-state publication has no recoverable reservation
    /// error: ordinary pressure waits inside `reserve`, while Closed,
    /// BatchTooLarge and outbox invariants mean the service cannot preserve
    /// its ownership/effect contract.
    pub(crate) async fn reserve_required_effects(
        &self,
        bytes: usize,
        context: &'static str,
    ) -> EffectPermit {
        match self.reserve_effects(bytes).await {
            Ok(permit) => permit,
            Err(error) => self.pipeline.runtime.fail_stop(context, &error),
        }
    }

    pub(crate) async fn reserve_critical_effects(
        &self,
        bytes: usize,
    ) -> Result<EffectPermit, EffectQueueError> {
        self.relay.effects.reserve_critical(bytes).await
    }

    fn publish_reserved_effects(
        &self,
        permit: EffectPermit,
        effects: Vec<TxPoolEffect>,
    ) -> Result<(), EffectQueueError> {
        let Some(batch) = EffectBatch::new(effects) else {
            drop(permit);
            return Ok(());
        };
        permit.commit(batch)
    }

    pub(crate) fn publish_required_reserved_effects(
        &self,
        permit: EffectPermit,
        effects: Vec<TxPoolEffect>,
        context: &'static str,
    ) {
        if let Err(error) = self.publish_reserved_effects(permit, effects) {
            self.pipeline.runtime.fail_stop(context, &error);
        }
    }

    pub(crate) async fn publish_effects(&self, effects: Vec<TxPoolEffect>) {
        let Some(batch) = EffectBatch::new(effects) else {
            return;
        };
        let permit = self
            .reserve_required_effects(
                batch.charge_bytes,
                "standalone tx-pool effect reservation failed",
            )
            .await;
        if let Err(error) = permit.commit(batch) {
            self.pipeline
                .runtime
                .fail_stop("reserved standalone tx-pool effect journal failed", &error);
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
mod tests {
    use super::*;
    use crate::callback::Callbacks;
    use crate::component::entry::TxEntry;
    use crate::network::{DummyTxPoolNetwork, TxPoolNetwork};
    use ckb_types::bytes::Bytes;
    use ckb_types::core::cell::CellMetaBuilder;
    use ckb_types::core::{Capacity, TransactionBuilder, cell::ResolvedTransaction};
    use ckb_types::packed::{CellInput, CellOutput, OutPoint};
    use ckb_types::prelude::{Builder, Entity};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn endpoints(tx_relay_sender: ckb_channel::Sender<TxVerificationResult>) -> EffectEndpoints {
        EffectEndpoints {
            network: Arc::new(DummyTxPoolNetwork),
            tx_relay_sender,
            failure_cancel: CancellationToken::new(),
        }
    }

    fn entry() -> TxEntry {
        TxEntry::dummy_resolve(
            TransactionBuilder::default().build(),
            0,
            Capacity::zero(),
            0,
        )
    }

    #[test]
    fn stable_effect_hash_detaches_from_transaction_backing() {
        let tx = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(Byte32::new([9; 32]), 0), 0))
            .witness(Bytes::from(vec![1; 64 * 1024]))
            .build();
        let backing = tx.data();
        let backing_start = backing.as_slice().as_ptr() as usize;
        let backing_end = backing_start + backing.as_slice().len();
        let shared_hash = tx.input_pts_iter().next().unwrap().tx_hash();
        let shared_start = shared_hash.as_slice().as_ptr() as usize;
        assert!(shared_start >= backing_start && shared_start < backing_end);

        let batch = EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
            tx_hash: shared_hash,
        })])
        .unwrap();
        let TxPoolEffect::Relay(TxVerificationResult::Reject { tx_hash }) = &batch.effects[0]
        else {
            panic!("expected relay rejection")
        };
        let stored_start = tx_hash.as_slice().as_ptr() as usize;
        assert!(stored_start < backing_start || stored_start >= backing_end);
    }

    #[test]
    fn pool_mutation_effect_formula_covers_every_generated_reject_shape() {
        let hash = Byte32::new([0xff; 32]);
        let out_point = OutPoint::new(hash.clone(), u32::MAX);
        let rejects = [
            Reject::Full(format!(
                "tx-pool total_tx_size {} overflows by add {}",
                usize::MAX,
                usize::MAX
            )),
            Reject::ExceededMaximumAncestorsCount,
            Reject::Resolve(OutPointError::Dead(out_point)),
            Reject::Resolve(OutPointError::InvalidHeader(hash.clone())),
            Reject::Expiry(u64::MAX),
            Reject::RBFRejected(format!("replaced by tx {hash}")),
            Reject::Invalidated(format!("invalidated by tx {hash}")),
        ];
        let transaction_bytes = minimum_serialized_transaction_bytes();
        let one_event_bound = max_pool_mutation_effect_bytes(transaction_bytes);
        for reject in rejects {
            assert!(bounded_pool_mutation_reject(&reject), "{reject}");
            let actual_worst_case = transaction_bytes
                .saturating_add(CALLBACK_SNAPSHOT_OVERHEAD_BYTES)
                .saturating_add(EFFECT_ENVELOPE_BYTES.saturating_mul(3))
                .saturating_add(reject.to_string().len().saturating_mul(2));
            assert!(
                actual_worst_case <= one_event_bound,
                "{reject}: {actual_worst_case} > {one_event_bound}"
            );
        }
    }

    #[test]
    fn callback_effect_drops_resolved_payload_at_journal_boundary() {
        let transaction = TransactionBuilder::default().build();
        let transaction_bytes = transaction.data().serialized_size_in_block();
        let resolved = Arc::new(ResolvedTransaction {
            transaction: transaction.clone(),
            resolved_cell_deps: vec![
                CellMetaBuilder::from_cell_output(
                    CellOutput::new_builder().build(),
                    Bytes::from(vec![0x5a; 1_000_000]),
                )
                .build(),
            ],
            resolved_inputs: Vec::new(),
            resolved_dep_groups: Vec::new(),
        });
        let resolved_owner = Arc::downgrade(&resolved);
        let entry = TxEntry::new(resolved, 42, Capacity::shannons(7), transaction_bytes);

        let effect = callback_accept(Arc::new(Callbacks::new()), entry, Status::Pending);

        assert!(
            resolved_owner.upgrade().is_none(),
            "the effect outbox must not retain resolved cell metadata"
        );
        assert!(
            effect.charge_bytes() < 4096,
            "callback charge must describe the compact snapshot, not the 1 MB resolved payload"
        );
        match effect {
            TxPoolEffect::Callback {
                event: CallbackEvent::Pending(snapshot),
                ..
            } => {
                assert_eq!(snapshot.transaction(), &transaction);
                assert_eq!(snapshot.cycles, 42);
                assert_eq!(snapshot.fee, Capacity::shannons(7));
            }
            _ => panic!("unexpected effect variant"),
        }
    }

    #[test]
    fn submit_effect_formula_covers_coordinator_settlement_and_bounded_ban() {
        let records = 3;
        let bound = max_submit_effect_bytes(0, minimum_serialized_transaction_bytes(), records);
        let mut effects = (0..records)
            .map(|seed| {
                TxPoolEffect::Relay(TxVerificationResult::Reject {
                    tx_hash: Byte32::new([seed as u8; 32]),
                })
            })
            .collect::<Vec<_>>();
        effects.push(TxPoolEffect::BanPeer {
            peer: 1.into(),
            duration: Duration::from_secs(1),
            reason: "x".repeat(MAX_COMMIT_BAN_REASON_BYTES),
        });
        let actual = EffectBatch::new(effects).unwrap().charge_bytes;
        assert!(actual <= bound, "{actual} > {bound}");

        let reject = Reject::Malformed("test".to_string(), "界".repeat(1024));
        let reason = bounded_commit_ban_reason(&reject);
        assert!(reason.len() <= MAX_COMMIT_BAN_REASON_BYTES);
        assert!(reason.is_char_boundary(reason.len()));
    }

    #[test]
    fn unknown_parent_effect_charges_hash_table_residency() {
        let parents = HashSet::from([Byte32::new([0; 32]), Byte32::new([0xff; 32])]);
        let effect = TxPoolEffect::Relay(TxVerificationResult::UnknownParents {
            peer: 1.into(),
            parents,
        });
        assert_eq!(
            effect.charge_bytes(),
            EFFECT_ENVELOPE_BYTES + 2 * UNKNOWN_PARENT_HASH_BYTES
        );
    }

    #[test]
    fn oversized_recent_reject_diagnostic_is_bounded_without_changing_original() {
        let reject = Reject::Full("界".repeat(2_000));
        let bounded = bounded_recent_reject(&reject);
        assert!(bounded.to_string().len() <= MAX_RECENT_REJECT_BYTES);
        assert!(serialized_recent_reject(&reject).len() <= MAX_RECENT_REJECT_BYTES);
        assert!(reject.to_string().len() > MAX_RECENT_REJECT_BYTES);
    }

    #[tokio::test]
    async fn recent_reject_database_write_runs_as_a_journaled_effect() {
        let temp = tempfile::Builder::new().tempdir().unwrap();
        let store = Arc::new(
            crate::component::recent_reject::RecentReject::build(temp.path(), 1, 100, -1).unwrap(),
        );
        let queue = Arc::new(EffectQueue::new(2, 1_000_000).unwrap());
        let (relay_tx, _relay_rx) = ckb_channel::bounded(1);
        let tx_hash = Byte32::new([0x42; 32]);
        queue
            .enqueue(
                EffectBatch::new(vec![TxPoolEffect::RecentReject {
                    store: Arc::clone(&store),
                    tx_hash: tx_hash.clone(),
                    serialized: serialized_recent_reject(&Reject::Expiry(42)),
                }])
                .unwrap(),
            )
            .await
            .unwrap();
        let publisher = tokio::spawn(run_effect_publisher(
            Arc::clone(&queue),
            endpoints(relay_tx),
        ));
        tokio::time::timeout(Duration::from_secs(1), queue.wait_idle())
            .await
            .unwrap();
        assert!(store.get(&tx_hash).unwrap().is_some());
        queue.close();
        publisher.await.unwrap();
    }

    #[tokio::test]
    async fn full_relayer_retains_fifo_head_and_outbox_charge() {
        let queue = Arc::new(EffectQueue::new(2, 1_000_000).unwrap());
        let (relay_tx, relay_rx) = ckb_channel::bounded(1);
        relay_tx
            .send(TxVerificationResult::Reject {
                tx_hash: Byte32::zero(),
            })
            .unwrap();
        let expected = Byte32::new([7; 32]);
        queue
            .enqueue(
                EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
                    tx_hash: expected.clone(),
                })])
                .unwrap(),
            )
            .await
            .unwrap();
        let publisher = tokio::spawn(run_effect_publisher(
            Arc::clone(&queue),
            endpoints(relay_tx),
        ));
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(queue.usage().batches, 1);
        relay_rx.try_recv().unwrap();

        tokio::time::timeout(Duration::from_secs(1), queue.wait_idle())
            .await
            .unwrap();
        match relay_rx.try_recv().unwrap() {
            TxVerificationResult::Reject { tx_hash } => assert_eq!(tx_hash, expected),
            other => panic!("unexpected relay result: {other:?}"),
        }
        queue.close();
        publisher.await.unwrap();
    }

    #[tokio::test]
    async fn close_drains_every_queued_batch_in_order() {
        let queue = Arc::new(EffectQueue::new(4, 1_000_000).unwrap());
        let (relay_tx, _relay_rx) = ckb_channel::bounded(4);
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        for value in [1usize, 2] {
            let mut callbacks = Callbacks::new();
            let observed = Arc::clone(&order);
            callbacks.register_pending(Box::new(move |_| {
                observed.lock().unwrap().push(value);
            }));
            queue
                .enqueue(
                    EffectBatch::new(vec![callback_accept(
                        Arc::new(callbacks),
                        entry(),
                        Status::Pending,
                    )])
                    .unwrap(),
                )
                .await
                .unwrap();
        }
        queue.close();
        run_effect_publisher(Arc::clone(&queue), endpoints(relay_tx)).await;
        assert_eq!(*order.lock().unwrap(), vec![1, 2]);
        assert_eq!(queue.usage().batches, 0);
    }

    #[tokio::test]
    async fn panicking_endpoint_is_quarantined_once_without_blocking_fifo() {
        struct PanickingNetwork {
            attempts: Arc<AtomicUsize>,
        }

        impl TxPoolNetwork for PanickingNetwork {
            fn ban_peer(&self, _peer: PeerIndex, _duration: Duration, _reason: String) {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                panic!("injected permanent network endpoint panic");
            }
        }

        let queue = Arc::new(EffectQueue::new(2, 1_000_000).unwrap());
        let (relay_tx, relay_rx) = ckb_channel::bounded(1);
        let attempts = Arc::new(AtomicUsize::new(0));
        let expected = Byte32::new([9; 32]);
        queue
            .enqueue(
                EffectBatch::new(vec![
                    TxPoolEffect::BanPeer {
                        peer: 1.into(),
                        duration: Duration::from_secs(1),
                        reason: "injected".to_owned(),
                    },
                    TxPoolEffect::Relay(TxVerificationResult::Reject {
                        tx_hash: expected.clone(),
                    }),
                ])
                .unwrap(),
            )
            .await
            .unwrap();

        let publisher = tokio::spawn(run_effect_publisher(
            Arc::clone(&queue),
            EffectEndpoints {
                network: Arc::new(PanickingNetwork {
                    attempts: Arc::clone(&attempts),
                }),
                tx_relay_sender: relay_tx,
                failure_cancel: CancellationToken::new(),
            },
        ));
        tokio::time::timeout(Duration::from_secs(1), queue.wait_idle())
            .await
            .expect("a permanently panicking endpoint must not retain the FIFO head");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        match relay_rx.try_recv().unwrap() {
            TxVerificationResult::Reject { tx_hash } => assert_eq!(tx_hash, expected),
            other => panic!("unexpected relay result: {other:?}"),
        }

        queue.close();
        publisher.await.unwrap();
    }

    #[tokio::test]
    async fn publisher_invariant_failure_closes_outbox_and_cancels_service() {
        let queue = Arc::new(EffectQueue::new(2, 1_000_000).unwrap());
        let (relay_tx, _relay_rx) = ckb_channel::bounded(1);
        queue
            .enqueue(
                EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
                    tx_hash: Byte32::zero(),
                })])
                .unwrap(),
            )
            .await
            .unwrap();
        // Simulate a second/failed publisher that left an active checkout.
        // The production publisher must fail the complete service instead of
        // logging and waiting forever behind that impossible state.
        {
            let mut state = queue.state.lock().unwrap();
            assert!(state.outbox.checkout().unwrap().is_some());
        }

        let failure_cancel = CancellationToken::new();
        let publisher = tokio::spawn(run_effect_publisher(
            Arc::clone(&queue),
            EffectEndpoints {
                network: Arc::new(DummyTxPoolNetwork),
                tx_relay_sender: relay_tx,
                failure_cancel: failure_cancel.clone(),
            },
        ));
        let result = tokio::time::timeout(Duration::from_secs(1), publisher)
            .await
            .expect("fatal publisher invariant must not hang");
        assert!(result.is_err());
        assert!(failure_cancel.is_cancelled());
        assert!(matches!(
            queue.reserve(1).await,
            Err(EffectQueueError::Closed)
        ));
    }

    #[tokio::test]
    async fn unused_pre_mutation_reservation_is_refunded_on_cancellation() {
        let queue = Arc::new(EffectQueue::new(1, 100).unwrap());
        let permit = queue.reserve(100).await.unwrap();
        assert_eq!(queue.usage().batches, 1);
        drop(permit);
        assert_eq!(queue.usage().batches, 0);

        // The exact same full-capacity reservation must be available again;
        // no abandoned async operation can strand an outbox credit.
        let permit = queue.reserve(100).await.unwrap();
        drop(permit);
        assert_eq!(queue.usage().batches, 0);
    }

    #[tokio::test]
    async fn binding_conservative_reservation_wakes_byte_capacity_waiter() {
        let queue = Arc::new(EffectQueue::new(2, 1_000).unwrap());
        let first = queue.reserve(1_000).await.unwrap();
        let waiting_queue = Arc::clone(&queue);
        let waiter = tokio::spawn(async move { waiting_queue.reserve(872).await.unwrap() });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        first
            .commit(
                EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
                    tx_hash: Byte32::zero(),
                })])
                .unwrap(),
            )
            .unwrap();

        let second = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("reservation shrink must wake the waiter")
            .unwrap();
        drop(second);
    }

    #[tokio::test]
    async fn ordinary_backpressure_cannot_consume_critical_reorg_headroom() {
        let queue = Arc::new(EffectQueue::new_with_critical_capacity(1, 128, 1, 1_000).unwrap());
        let ordinary = queue.reserve(128).await.unwrap();
        let waiting_queue = Arc::clone(&queue);
        let ordinary_waiter = tokio::spawn(async move { waiting_queue.reserve(1).await });
        tokio::task::yield_now().await;
        assert!(!ordinary_waiter.is_finished());

        let critical = tokio::time::timeout(Duration::from_secs(1), queue.reserve_critical(1_000))
            .await
            .expect("ordinary saturation must leave critical headroom")
            .unwrap();

        drop(critical);
        drop(ordinary);
        let waiter = tokio::time::timeout(Duration::from_secs(1), ordinary_waiter)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(waiter);
    }
}
