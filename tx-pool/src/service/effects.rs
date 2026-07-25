//! Bounded publication of stable-state tx-pool effects.
//!
//! State owners enqueue immutable batches after their authoritative mutation.
//! A single publisher, independent of the controller dispatcher, preserves
//! batch order and contains callback re-entry. The outbox charges queued and
//! active payloads continuously; a full outbox applies backpressure without
//! retaining an unbounded task/channel backlog.

use crate::callback::CallbackEvent;
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
            tokio::pin!(space);
            // `notify_waiters` does not store a permit. Register before the
            // capacity check so a release/close between the check and await
            // cannot leave this producer asleep forever.
            space.as_mut().enable();
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

    /// Required stable-state publication has no recoverable reservation
    /// error: ordinary pressure waits inside `reserve`, while Closed,
    /// BatchTooLarge and outbox invariants mean the service cannot preserve
    /// its ownership/effect contract.
    pub(crate) async fn reserve_required_effects(
        &self,
        bytes: usize,
        context: &'static str,
    ) -> EffectPermit {
        match self.relay.effects.reserve(bytes).await {
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
#[path = "tests/effects.rs"]
mod tests;
