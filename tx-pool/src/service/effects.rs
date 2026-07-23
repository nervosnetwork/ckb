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
use crate::component::entry::TxEntry;
use crate::component::pool_map::Status;
use crate::error::Reject;
use crate::network::TxPoolNetworkHandle;
use crate::service::TxPoolService;
use crate::service::TxVerificationResult;
use ckb_channel::TrySendError;
use ckb_logger::{error, info, warn};
use ckb_network::PeerIndex;
use ckb_types::packed::Byte32;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

pub(crate) const EFFECT_ENVELOPE_BYTES: usize = 128;
/// A reorg can reject every resident transaction. Each rejection may retain
/// the transaction for a callback plus a relay envelope and a bounded reject
/// description. Eight times the pool's serialized-byte limit is a
/// conservative whole-pool reservation; commit shrinks it to the actual
/// immutable batch charge before publication.
pub(crate) const REORG_EFFECT_CAPACITY_MULTIPLIER: usize = 8;

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
}

impl TxPoolEffect {
    fn charge_bytes(&self) -> usize {
        match self {
            Self::Callback {
                event: CallbackEvent::Pending(entry),
                ..
            }
            | Self::Callback {
                event: CallbackEvent::Proposed(entry),
                ..
            } => EFFECT_ENVELOPE_BYTES
                .saturating_add(entry.transaction().data().serialized_size_in_block()),
            Self::Callback {
                event: CallbackEvent::Reject(entry, reject),
                ..
            } => EFFECT_ENVELOPE_BYTES
                .saturating_add(entry.transaction().data().serialized_size_in_block())
                .saturating_add(reject.to_string().len()),
            Self::Relay(TxVerificationResult::UnknownParents { parents, .. }) => {
                EFFECT_ENVELOPE_BYTES
                    .saturating_add(parents.len().saturating_mul(std::mem::size_of::<Byte32>()))
            }
            Self::Relay(TxVerificationResult::Ok { .. } | TxVerificationResult::Reject { .. }) => {
                EFFECT_ENVELOPE_BYTES
            }
            Self::BanPeer { reason, .. } => EFFECT_ENVELOPE_BYTES.saturating_add(reason.len()),
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
            state.outbox.cancel(reservation)
        };
        if let Err(error) = cancelled {
            error!("failed to cancel unused effect reservation: {:?}", error);
        }
        self.queue.space.notify_waiters();
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

    pub(crate) fn commit(
        self: &Arc<Self>,
        mut permit: EffectPermit,
        batch: EffectBatch,
    ) -> Result<(), EffectQueueError> {
        if !Arc::ptr_eq(self, &permit.queue) {
            return Err(EffectQueueError::Invariant(
                EffectOutboxError::MissingReservation,
            ));
        }
        if batch.charge_bytes > permit.bytes {
            return Err(EffectQueueError::BatchTooLarge {
                bytes: batch.charge_bytes,
                max_bytes: permit.bytes,
            });
        }
        let reservation = permit
            .reservation
            .take()
            .ok_or(EffectQueueError::Invariant(
                EffectOutboxError::MissingReservation,
            ))?;
        let batch = Arc::new(batch);
        let result = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let result = state
                .outbox
                .shrink_reservation(reservation, batch.charge_bytes)
                .and_then(|_| state.outbox.bind_sequence(reservation))
                .and_then(|_| state.outbox.enqueue(reservation, batch));
            if result.is_err() {
                let _ = state.outbox.cancel(reservation);
            }
            result
        };
        if let Err(error) = result {
            self.space.notify_waiters();
            return Err(EffectQueueError::Invariant(error));
        }
        // A conservative permit may have been shrunk substantially above.
        // Wake capacity waiters now; waiting for this batch to publish can
        // deadlock when its head is a causal barrier released by one of those
        // waiters.
        self.space.notify_waiters();
        self.ready.notify_one();
        Ok(())
    }

    pub(crate) async fn enqueue(
        self: &Arc<Self>,
        batch: EffectBatch,
    ) -> Result<(), EffectQueueError> {
        let permit = self.reserve(batch.charge_bytes).await?;
        self.commit(permit, batch)
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
        }
        PublishOne::Complete
    }
}

/// Drain until `close` has been called and every queued/active batch is done.
pub(crate) async fn run_effect_publisher(queue: Arc<EffectQueue>, endpoints: EffectEndpoints) {
    loop {
        let ready = queue.ready.notified();
        let (closed, sequence, batch) = {
            let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
            let closed = state.closed;
            let sequence = match state.outbox.checkout() {
                Ok(sequence) => sequence,
                Err(EffectOutboxError::PublisherBusy) => None,
                Err(error) => {
                    error!("effect outbox checkout invariant failure: {:?}", error);
                    None
                }
            };
            let batch = sequence.and_then(|sequence| {
                state
                    .outbox
                    .active_effect(sequence)
                    .map(Arc::clone)
                    .map_err(|error| error!("effect outbox active invariant failure: {:?}", error))
                    .ok()
            });
            (closed, sequence, batch)
        };

        let Some(sequence) = sequence else {
            let empty = {
                let state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
                state.outbox.usage().batches == 0
            };
            if closed && empty {
                break;
            }
            queue.quiescent.notify_waiters();
            ready.await;
            continue;
        };
        let Some(batch) = batch else {
            warn!(
                "effect publisher checked out sequence {} without a batch",
                sequence
            );
            continue;
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
                    // The cursor advances only after a normal return. Retain
                    // this head effect and retry with bounded backoff.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
        if batch.is_complete() {
            let completed = {
                let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
                state.outbox.complete_active(sequence)
            };
            if let Err(error) = completed {
                error!("effect outbox completion invariant failure: {:?}", error);
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
        Status::Proposed => CallbackEvent::Proposed(entry),
        Status::Pending | Status::Gap => CallbackEvent::Pending(entry),
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
        event: CallbackEvent::Reject(entry, reject),
    }
}

impl TxPoolService {
    pub(crate) fn unknown_parents_effect_bytes(parent_count: usize) -> usize {
        EFFECT_ENVELOPE_BYTES
            .saturating_add(parent_count.saturating_mul(std::mem::size_of::<Byte32>()))
    }

    pub(crate) fn max_submit_effect_bytes(&self) -> usize {
        let by_pool = self.pool.tx_pool_config.max_tx_pool_size.saturating_mul(4);
        let by_bounded_removals = (self.pool.consensus.max_block_bytes() as usize)
            .saturating_mul(crate::constants::MAX_RBF_REPLACEMENT_CANDIDATES.saturating_add(4));
        by_pool.min(by_bounded_removals).max(4096)
    }

    pub(crate) fn max_reorg_effect_bytes(&self) -> usize {
        self.pool
            .tx_pool_config
            .max_tx_pool_size
            .saturating_mul(REORG_EFFECT_CAPACITY_MULTIPLIER)
            .max(4096)
    }

    pub(crate) async fn reserve_effects(
        &self,
        bytes: usize,
    ) -> Result<EffectPermit, EffectQueueError> {
        self.relay.effects.reserve(bytes).await
    }

    pub(crate) async fn reserve_critical_effects(
        &self,
        bytes: usize,
    ) -> Result<EffectPermit, EffectQueueError> {
        self.relay.effects.reserve_critical(bytes).await
    }

    pub(crate) fn publish_reserved_effects(
        &self,
        permit: EffectPermit,
        effects: Vec<TxPoolEffect>,
    ) -> Result<(), EffectQueueError> {
        let Some(batch) = EffectBatch::new(effects) else {
            drop(permit);
            return Ok(());
        };
        self.relay.effects.commit(permit, batch)
    }

    pub(crate) async fn publish_effects(&self, effects: Vec<TxPoolEffect>) {
        let Some(batch) = EffectBatch::new(effects) else {
            return;
        };
        if let Err(error) = self.relay.effects.enqueue(batch).await {
            // Closing happens only after all state workers have quiesced. An
            // enqueue error before then is an invariant/configuration failure,
            // not permission to run the external effect in the caller.
            error!("failed to journal tx-pool stable-state effect: {:?}", error);
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
        if reject.should_recorded() {
            self.record_recent_reject(&entry.transaction().hash(), &reject);
        }
        let mut effects = Vec::new();
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

    pub(crate) fn record_recent_reject(&self, tx_hash: &Byte32, reject: &Reject) {
        if let Some(store) = &self.aux.recent_reject
            && let Err(error) = store.put(tx_hash, reject.clone())
        {
            error!(
                "failed to record recent reject {} {}: {}",
                tx_hash, reject, error
            );
        }
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
    use crate::network::DummyTxPoolNetwork;
    use ckb_types::core::{Capacity, TransactionBuilder};

    fn endpoints(tx_relay_sender: ckb_channel::Sender<TxVerificationResult>) -> EffectEndpoints {
        EffectEndpoints {
            network: Arc::new(DummyTxPoolNetwork),
            tx_relay_sender,
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

        queue
            .commit(
                first,
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
