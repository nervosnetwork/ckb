//! Immutable, bounded notices are appended with the owner commit and published
//! by one task after its guards open. There is no preallocated sequence gap.
use super::{
    budget::Limits,
    model::{DependencyKey, Entry, Error, FullReason, Phase, Status},
    relay::{AuthorityRelaySink, RelayMailboxDisposition},
};
use crate::{
    callback::{CallbackEvent, Callbacks},
    component::{entry::TxEntrySnapshot, recent_reject::RecentReject},
    constants::MAX_TX_POOL_REJECT_DESCRIPTION_BYTES,
    error::Reject,
    network::TxPoolNetworkHandle,
    service::TxVerificationResult,
    util::{block_offload, compact_packed},
};
use ckb_fee_estimator::FeeEstimator;
use ckb_jsonrpc_types::PoolTransactionReject;
use ckb_network::PeerIndex;
use ckb_types::{core::BlockView, packed::Byte32};
use ckb_util::parking_lot::Mutex;
use std::{
    collections::{BTreeMap, VecDeque},
    mem::size_of,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;

// Bound one endpoint handoff; each batch still settles and releases separately.
const PUBLISH_BATCH_LIMIT: usize = 32;

#[cfg(test)]
#[path = "tests/notice.rs"]
mod tests;

/// Endpoint order is recent rejection, callback, ban, relay, chain observation.
/// These values are fully built before Apply and never consult current owners.
#[derive(Clone, Default)]
pub(super) struct Effect {
    rejection: Option<crate::metrics::RejectionClass>,
    recent: Option<(Byte32, Reject, String)>,
    callback: Option<CallbackEvent>,
    ban: Option<(PeerIndex, Instant, String)>,
    relay: Option<TxVerificationResult>,
    blocks: Vec<Arc<BlockView>>,
}
impl Effect {
    pub(super) fn accepted(
        entry: TxEntrySnapshot,
        status: Status,
        peer: Option<PeerIndex>,
    ) -> Self {
        let hash = compact_packed(&entry.transaction.hash());
        Self {
            relay: Some(TxVerificationResult::Ok {
                original_peer: peer,
                tx_hash: hash,
            }),
            ..Self::projected(entry, status)
        }
    }
    pub(super) fn projected(entry: TxEntrySnapshot, status: Status) -> Self {
        Self {
            callback: Some(match status {
                Status::Proposed => CallbackEvent::Proposed(entry),
                Status::Pending | Status::Gap => CallbackEvent::Pending(entry),
            }),
            ..Self::default()
        }
    }
    pub(super) fn relay(result: TxVerificationResult) -> Self {
        Self {
            relay: Some(result),
            ..Self::default()
        }
    }
    pub(super) fn waiting(entry: &Entry) -> Option<Self> {
        let Phase::Waiting(keys) = &entry.phase else {
            return None;
        };
        let peer = entry.source.residency_peer()?;
        Some(Self::relay(TxVerificationResult::UnknownParents {
            peer,
            parents: keys
                .iter()
                .filter_map(|key| match key {
                    DependencyKey::Cell(point) => Some(compact_packed(&point.tx_hash())),
                    DependencyKey::Header(_) => None,
                })
                .collect(),
        }))
    }
    pub(super) fn blocks(blocks: Vec<Arc<BlockView>>) -> Self {
        Self {
            blocks,
            ..Self::default()
        }
    }
    pub(super) fn banned(
        hash: &Byte32,
        reject: Reject,
        peer: PeerIndex,
        deadline: Instant,
    ) -> Result<Self, Error> {
        let reason = bounded_ban_reason(&reject);
        Ok(Self {
            ban: Some((peer, deadline, reason)),
            relay: Some(TxVerificationResult::GenerationReset),
            ..Self::rejected(hash, reject, None, false)?
        })
    }
    #[cfg(test)]
    pub(super) fn relay_result(&self) -> Option<&TxVerificationResult> {
        self.relay.as_ref()
    }
    #[cfg(test)]
    pub(super) fn callback(&self) -> Option<&CallbackEvent> {
        self.callback.as_ref()
    }
    pub(super) fn rejected(
        hash: &Byte32,
        reject: Reject,
        accepted: Option<TxEntrySnapshot>,
        relay: bool,
    ) -> Result<Self, Error> {
        // Preserve policy bits before detaching dynamically owned diagnostics.
        let rejection = Some(crate::metrics::RejectionClass::from_reject(&reject));
        let record = reject.should_recorded();
        let negative =
            relay && reject.is_allowed_relay() && !matches!(reject, Reject::Duplicated(_));
        let reject = bound_reject_diagnostic(reject);
        let recent = if record {
            Some((
                compact_packed(hash),
                reject.clone(),
                serialized_recent_reject(&reject)?,
            ))
        } else {
            None
        };
        Ok(Self {
            rejection,
            recent,
            callback: accepted.map(|entry| CallbackEvent::Reject(entry, reject)),
            relay: negative.then(|| TxVerificationResult::Reject {
                tx_hash: compact_packed(hash),
            }),
            ..Self::default()
        })
    }
    pub(super) fn reset() -> Self {
        Self {
            relay: Some(TxVerificationResult::GenerationReset),
            ..Self::default()
        }
    }
    fn bytes(&self) -> Option<usize> {
        const CALLBACK_METADATA_BYTES: usize =
            size_of::<TxEntrySnapshot>() + MAX_TX_POOL_REJECT_DESCRIPTION_BYTES;
        let mut bytes = size_of::<Self>().checked_add(256)?;
        if let Some((_, _, serialized)) = &self.recent {
            bytes = bytes
                .checked_add(128 + MAX_TX_POOL_REJECT_DESCRIPTION_BYTES)?
                .checked_add(serialized.capacity())?;
        }
        if let Some(event) = &self.callback {
            let entry = match event {
                CallbackEvent::Pending(e)
                | CallbackEvent::Proposed(e)
                | CallbackEvent::Reject(e, _) => e,
            };
            bytes = bytes
                .checked_add(entry.transaction.data().total_size())?
                .checked_add(CALLBACK_METADATA_BYTES)?;
        }
        if let Some((_, _, reason)) = &self.ban {
            bytes = bytes.checked_add(reason.capacity())?;
        }
        if let Some(TxVerificationResult::UnknownParents { parents, .. }) = &self.relay {
            bytes = bytes.checked_add(parents.capacity().checked_mul(128)?)?;
        }
        bytes = bytes.checked_add(
            self.blocks
                .capacity()
                .checked_mul(size_of::<Arc<BlockView>>())?,
        )?;
        for block in &self.blocks {
            bytes = bytes.checked_add(block.data().total_size())?;
        }
        Some(bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    Remote,
    Trusted,
    Critical,
}
impl Class {
    fn index(self) -> usize {
        match self {
            Self::Remote => 0,
            Self::Trusted => 1,
            Self::Critical => 2,
        }
    }
}
#[derive(Clone, Copy, Default)]
struct Charge {
    items: usize,
    bytes: usize,
}
impl Charge {
    fn add(self, rhs: Self) -> Option<Self> {
        Some(Self {
            items: self.items.checked_add(rhs.items)?,
            bytes: self.bytes.checked_add(rhs.bytes)?,
        })
    }
    fn sub(self, rhs: Self) -> Option<Self> {
        Some(Self {
            items: self.items.checked_sub(rhs.items)?,
            bytes: self.bytes.checked_sub(rhs.bytes)?,
        })
    }
    fn fits(self, rhs: Self) -> bool {
        self.items <= rhs.items && self.bytes <= rhs.bytes
    }
}

pub(super) struct Batch {
    effects: Vec<Effect>,
    charge: Charge,
    class: Class,
    ready: AtomicBool,
    published: AtomicBool,
    completed: Notify,
}
impl Batch {
    /// Open publication only after Store::apply has released its commit guards.
    /// An appended batch keeps its FIFO position while waiting for activation.
    pub(super) fn activate(&self, outbox: &Outbox) {
        self.ready.store(true, Ordering::Release);
        outbox.changed.notify_one();
    }
    /// Wait for the synchronous endpoint pass and removal from the outbox.
    /// Endpoint failure policies may omit delivery; success does not acknowledge
    /// downstream relay consumption. Cancelling this waiter does not remove or
    /// cancel the committed batch.
    pub(super) async fn wait(&self, outbox: &Outbox) -> Result<(), Error> {
        loop {
            // Each notification is created before checking its factual flag.
            // notify_waiters is observed even if it precedes the first poll.
            let completed = self.completed.notified();
            let failed = outbox.failed.notified();
            if self.published.load(Ordering::Acquire) {
                return Ok(());
            }
            if outbox.faulted.load(Ordering::Acquire) {
                return Err(Error::Fault("notice publisher"));
            }
            tokio::select! {
                biased;
                _ = completed => {},
                _ = failed => {},
            }
        }
    }
}
struct State {
    queue: VecDeque<Arc<Batch>>,
    usage: [Charge; 3],
    pending: BTreeMap<Byte32, Weak<Batch>>,
    closed: bool,
}
pub(super) struct Outbox {
    state: Mutex<State>,
    limits: [Charge; 3],
    batch_bytes: [usize; 3],
    batch_effects: [usize; 3],
    faulted: Arc<AtomicBool>,
    pub(super) changed: Notify,
    pub(super) failed: Notify,
    pub(super) room: Notify,
}
pub(super) struct Reservation {
    outbox: Arc<Outbox>,
    batch: Arc<Batch>,
    appended: bool,
}

impl Outbox {
    pub(super) fn new(limits: &Limits, faulted: Arc<AtomicBool>) -> Result<Arc<Self>, Error> {
        let arithmetic = || Error::Full("notice configuration arithmetic".into());
        let effects = crate::constants::MAX_POOL_MUTATION_CANDIDATES
            .checked_add(1)
            .ok_or_else(arithmetic)?;
        const PER_EFFECT: usize = size_of::<Effect>()
            + size_of::<TxEntrySnapshot>()
            + 768
            + MAX_TX_POOL_REJECT_DESCRIPTION_BYTES * 3;
        let admission_payload = limits
            .accepted
            .serialized
            .checked_add(limits.max_block_bytes)
            .ok_or_else(arithmetic)?
            .min(
                effects
                    .checked_mul(limits.max_block_bytes)
                    .ok_or_else(arithmetic)?,
            );
        let admission = admission_payload
            .checked_add(effects.checked_mul(PER_EFFECT).ok_or_else(arithmetic)?)
            .ok_or_else(arithmetic)?
            .max(4096);
        let all_effects = limits.max_owners.checked_add(1).ok_or_else(arithmetic)?;
        let critical = limits
            .accepted
            .bytes
            .checked_add(limits.pipeline.bytes)
            .and_then(|b| b.checked_add(limits.max_block_bytes))
            .and_then(|b| b.checked_add(all_effects.checked_mul(PER_EFFECT)?))
            .ok_or_else(arithmetic)?
            .max(4096);
        let parents = limits
            .per_job
            .edges
            .checked_mul(128)
            .and_then(|b| b.checked_add(PER_EFFECT))
            .ok_or_else(arithmetic)?;
        let remote_bytes = limits
            .accepted
            .serialized
            .checked_add(limits.pipeline.bytes)
            .and_then(|b| b.checked_mul(2))
            .ok_or_else(arithmetic)?
            .max(admission)
            .max(parents);
        let remote = Charge {
            items: crate::constants::EFFECT_JOURNAL_REMOTE_MAX_BATCHES,
            bytes: remote_bytes,
        };
        let ordinary = remote
            .add(Charge {
                items: crate::constants::EFFECT_TRUSTED_HEADROOM_BATCHES,
                bytes: admission,
            })
            .ok_or_else(arithmetic)?;
        let total = ordinary
            .add(Charge {
                items: 1,
                bytes: critical,
            })
            .ok_or_else(arithmetic)?;
        let mut queue = VecDeque::new();
        queue
            .try_reserve_exact(total.items)
            .map_err(|_| Error::Full("notice allocation".into()))?;
        Ok(Arc::new(Self {
            state: Mutex::new(State {
                queue,
                usage: [Charge::default(); 3],
                pending: BTreeMap::new(),
                closed: false,
            }),
            limits: [remote, ordinary, total],
            batch_bytes: [remote_bytes, admission, critical],
            batch_effects: [effects, effects, all_effects],
            faulted,
            changed: Notify::new(),
            failed: Notify::new(),
            room: Notify::new(),
        }))
    }
    #[expect(
        clippy::indexing_slicing,
        reason = "Class::index returns 0, 1 or 2 for the three fixed capacity arrays."
    )]
    pub(super) fn reserve(
        self: &Arc<Self>,
        effects: Vec<Effect>,
        class: Class,
    ) -> Result<Option<Reservation>, Error> {
        if effects.is_empty() {
            return Ok(None);
        }
        let base = effects
            .capacity()
            .checked_mul(size_of::<Effect>())
            .and_then(|b| b.checked_add(size_of::<Batch>()))
            .ok_or(Error::Full("notice vector arithmetic".into()))?;
        let bytes = effects
            .iter()
            .try_fold(base, |sum, effect| sum.checked_add(effect.bytes()?))
            .ok_or(Error::Full("notice byte arithmetic".into()))?;
        let index = class.index();
        if effects.len() > self.batch_effects[index] || bytes > self.batch_bytes[index] {
            return Err(Error::Full("indivisible notice batch".into()));
        }
        let charge = Charge { items: 1, bytes };
        let batch = Arc::new(Batch {
            effects,
            charge,
            class,
            ready: AtomicBool::new(false),
            published: AtomicBool::new(false),
            completed: Notify::new(),
        });
        let mut state = self.state.lock();
        if state.closed {
            return Err(Error::Closed);
        }
        let mut projected = state.usage;
        for (usage, limit) in projected.iter_mut().zip(self.limits).skip(index) {
            *usage = usage
                .add(charge)
                .filter(|value| value.fits(limit))
                .ok_or(Error::Full(FullReason::NoticeOutbox))?;
        }
        state.usage = projected;
        drop(state);
        Ok(Some(Reservation {
            outbox: Arc::clone(self),
            batch,
            appended: false,
        }))
    }
    pub(super) fn pending_reject(&self, hash: &Byte32) -> Option<Reject> {
        let batch = self.state.lock().pending.get(hash)?.upgrade()?;
        batch.effects.iter().rev().find_map(|effect| {
            effect
                .recent
                .as_ref()
                .filter(|(key, _, _)| key == hash)
                .map(|(_, reject, _)| reject.clone())
        })
    }
    fn release(&self, batch: &Batch, state: &mut State) -> bool {
        let mut failed = false;
        for usage in state.usage.iter_mut().skip(batch.class.index()) {
            if let Some(next) = usage.sub(batch.charge) {
                *usage = next;
            } else {
                self.faulted.store(true, Ordering::Release);
                failed = true;
            }
        }
        failed
    }
    pub(super) fn publish_metrics(&self) {
        let snapshot = {
            let state = self.state.lock();
            let [remote, ordinary, total] = state.usage;
            crate::metrics::EffectUsage {
                remote_batches: remote.items,
                remote_bytes: remote.bytes,
                ordinary_batches: ordinary.items,
                ordinary_bytes: ordinary.bytes,
                total_batches: total.items,
                total_bytes: total.bytes,
            }
        };
        snapshot.publish();
    }
    pub(super) fn close(&self) {
        self.state.lock().closed = true;
        self.changed.notify_one();
        self.room.notify_waiters();
    }
    pub(super) fn drained(&self) -> bool {
        let state = self.state.lock();
        state.closed && state.queue.is_empty() && state.usage[2].items == 0
    }
    fn publish_ready(&self, endpoints: &mut Endpoints) -> Result<bool, Error> {
        let (head, count) = {
            let state = self.state.lock();
            let Some(head) = state
                .queue
                .front()
                .filter(|batch| batch.ready.load(Ordering::Acquire))
            else {
                return Ok(false);
            };
            let count = state
                .queue
                .iter()
                .take(PUBLISH_BATCH_LIMIT)
                .take_while(|batch| batch.ready.load(Ordering::Acquire))
                .count();
            (Arc::clone(head), count)
        };
        let offload = count > 1
            && head
                .effects
                .iter()
                .filter_map(|effect| effect.callback.as_ref())
                .any(|event| endpoints.callback_enabled(event));
        // One publisher owns removal, and readiness only opens. Transfer the
        // selected head directly; groups still release each earlier batch
        // before running a later endpoint.
        let mut next = Some(head);
        #[cfg(feature = "profiling")]
        let _group_span = tracing::trace_span!(
            target: "ckb_tx_pool_profile", "tx_pool.publisher.group"
        )
        .entered();
        #[cfg(feature = "profiling")]
        {
            // Count the selected ready prefix without holding the FIFO lock.
            // These creation-only markers have no entered-duration meaning.
            let _prefix_span = match count {
                1 => tracing::trace_span!(
                    target: "ckb_tx_pool_profile", "tx_pool.publisher.ready_1"
                ),
                2..=4 => tracing::trace_span!(
                    target: "ckb_tx_pool_profile", "tx_pool.publisher.ready_2_4"
                ),
                5..=8 => tracing::trace_span!(
                    target: "ckb_tx_pool_profile", "tx_pool.publisher.ready_5_8"
                ),
                9..=16 => tracing::trace_span!(
                    target: "ckb_tx_pool_profile", "tx_pool.publisher.ready_9_16"
                ),
                _ => tracing::trace_span!(
                    target: "ckb_tx_pool_profile", "tx_pool.publisher.ready_17_32"
                ),
            };
        }
        let mut publish = || {
            for remaining in (0..count).rev() {
                let batch = next.take().ok_or(Error::Fault("notice FIFO head"))?;
                // No await in a batch terminal: cancellation cannot replay a prefix.
                for effect in &batch.effects {
                    endpoints.publish(effect);
                }
                let mut state = self.state.lock();
                if !state
                    .queue
                    .front()
                    .is_some_and(|head| Arc::ptr_eq(head, &batch))
                {
                    return Err(Error::Fault("notice FIFO head"));
                }
                for effect in &batch.effects {
                    if let Some((hash, _, _)) = &effect.recent
                        && state
                            .pending
                            .get(hash)
                            .is_some_and(|old| old.ptr_eq(&Arc::downgrade(&batch)))
                    {
                        state.pending.remove(hash);
                    }
                }
                let retired = state.queue.pop_front();
                let failed = self.release(&batch, &mut state);
                if remaining > 0 {
                    next = state
                        .queue
                        .front()
                        .filter(|batch| batch.ready.load(Ordering::Acquire))
                        .cloned();
                }
                drop(state);
                batch.published.store(true, Ordering::Release);
                batch.completed.notify_waiters();
                if failed {
                    self.failed.notify_waiters();
                }
                self.room.notify_waiters();
                drop(retired);
            }
            Ok(true)
        };
        if offload {
            #[cfg(feature = "profiling")]
            let _offload_span = tracing::trace_span!(
                target: "ckb_tx_pool_profile", "tx_pool.publisher.offload"
            )
            .entered();
            block_offload(publish)
        } else {
            publish()
        }
    }
    pub(super) async fn run(self: Arc<Self>, mut endpoints: Endpoints) -> Result<(), Error> {
        struct Completion {
            outbox: Arc<Outbox>,
            done: bool,
        }
        impl Drop for Completion {
            fn drop(&mut self) {
                if !self.done {
                    crate::metrics::record_failure(
                        crate::metrics::FailureBoundary::EffectPublisher,
                    );
                    self.outbox.faulted.store(true, Ordering::Release);
                    self.outbox.failed.notify_waiters();
                    self.outbox.room.notify_waiters();
                }
            }
        }
        let mut completion = Completion {
            outbox: Arc::clone(&self),
            done: false,
        };
        loop {
            let changed = self.changed.notified();
            if self.publish_ready(&mut endpoints)? {
                continue;
            }
            if self.drained() {
                completion.done = true;
                return Ok(());
            }
            if self.faulted.load(Ordering::Acquire) {
                return Err(Error::Fault("notice publisher"));
            }
            changed.await;
        }
    }
}
impl Reservation {
    /// Called only after the final owner cut has no expected failure left.
    pub(super) fn append(mut self) -> Arc<Batch> {
        let batch = Arc::clone(&self.batch);
        self.appended = true;
        let mut state = self.outbox.state.lock();
        for effect in &batch.effects {
            if let Some((hash, _, _)) = &effect.recent {
                state
                    .pending
                    .insert(compact_packed(hash), Arc::downgrade(&batch));
            }
        }
        state.queue.push_back(Arc::clone(&batch));
        batch
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.appended {
            let mut state = self.outbox.state.lock();
            let failed = self.outbox.release(&self.batch, &mut state);
            drop(state);
            if failed {
                self.outbox.failed.notify_waiters();
            }
            self.outbox.room.notify_waiters();
            self.outbox.changed.notify_one();
        }
    }
}

#[derive(Default)]
enum RecentWrites {
    #[default]
    Available,
    CoolingDown(tokio::time::Instant),
    Disabled,
}
impl RecentWrites {
    fn write(&mut self, operation: impl FnOnce() -> Result<(), ckb_error::AnyError>) {
        match self {
            Self::Disabled => return,
            Self::CoolingDown(failed) if failed.elapsed() < Duration::from_secs(1) => return,
            _ => {}
        }
        match run_endpoint("recent_reject", operation) {
            Some(Ok(())) => {
                if matches!(self, Self::CoolingDown(_)) {
                    ckb_logger::info!(
                        "tx-pool recent_reject writes resumed; omitted records are not replayed"
                    );
                }
                *self = Self::Available;
            }
            Some(Err(error)) => {
                // Start at failure completion, even when the write itself was slow.
                *self = Self::CoolingDown(tokio::time::Instant::now());
                crate::metrics::record_failure(crate::metrics::FailureBoundary::EffectPublisher);
                ckb_logger::warn!(
                    "tx-pool recent_reject write failed; new writes omitted for one second: {}",
                    bounded_text(error.to_string(), 1024)
                );
            }
            None => *self = Self::Disabled,
        }
    }
}

pub(crate) struct Endpoints {
    network: TxPoolNetworkHandle,
    relay: AuthorityRelaySink,
    callbacks: Arc<Callbacks>,
    recent: Option<Arc<RecentReject>>,
    recent_writes: RecentWrites,
    estimator: FeeEstimator,
    callbacks_disabled: bool,
    ban_disabled: bool,
    relay_disabled: bool,
    estimator_disabled: bool,
}
impl Endpoints {
    pub(crate) fn new(
        network: TxPoolNetworkHandle,
        relay: AuthorityRelaySink,
        callbacks: Arc<Callbacks>,
        recent: Option<Arc<RecentReject>>,
        estimator: FeeEstimator,
    ) -> Self {
        Self {
            network,
            relay,
            callbacks,
            recent,
            recent_writes: RecentWrites::default(),
            estimator,
            callbacks_disabled: false,
            ban_disabled: false,
            relay_disabled: false,
            estimator_disabled: false,
        }
    }
    fn callback_enabled(&self, event: &CallbackEvent) -> bool {
        !self.callbacks_disabled
            && match event {
                CallbackEvent::Pending(_) => self.callbacks.pending.is_some(),
                CallbackEvent::Proposed(_) => self.callbacks.proposed.is_some(),
                CallbackEvent::Reject(_, _) => self.callbacks.reject.is_some(),
            }
    }
    fn publish(&mut self, effect: &Effect) {
        #[cfg(feature = "profiling")]
        let _span = tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.effects.publish")
            .entered();
        if let Some(rejection) = effect.rejection {
            rejection.record();
        }
        if let (Some(store), Some((hash, _, serialized))) = (&self.recent, &effect.recent) {
            self.recent_writes
                .write(|| store.put_serialized(hash, serialized));
        }
        if let Some(event) = &effect.callback
            && self.callback_enabled(event)
            && run_endpoint("callbacks", || {
                crate::callback::with_callback_context(|| self.callbacks.publish(event))
            })
            .is_none()
        {
            self.callbacks_disabled = true;
        }
        if !self.ban_disabled
            && let Some((peer, deadline, reason)) = &effect.ban
            && run_endpoint("network_ban", || {
                if let Some(duration) = deadline.checked_duration_since(Instant::now()) {
                    self.network.ban_peer(*peer, duration, reason.clone());
                }
            })
            .is_none()
        {
            self.ban_disabled = true;
        }
        if !self.relay_disabled
            && let Some(result) = &effect.relay
            && matches!(
                self.relay.publish(result.clone()),
                RelayMailboxDisposition::Disconnected
            )
        {
            self.relay_disabled = true;
            crate::metrics::record_failure(crate::metrics::FailureBoundary::EffectPublisher);
            ckb_logger::warn!(
                "tx-pool relay receiver disconnected; relay disabled for publisher lifetime"
            );
        }
        if !self.estimator_disabled {
            for block in &effect.blocks {
                if run_endpoint("fee_estimator", || self.estimator.commit_block(block)).is_none() {
                    self.estimator_disabled = true;
                    break;
                }
            }
        }
    }
}
fn run_endpoint<T>(name: &'static str, operation: impl FnOnce() -> T) -> Option<T> {
    block_offload(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
            .map_err(|_| {
                crate::metrics::record_failure(crate::metrics::FailureBoundary::EffectPublisher);
                ckb_logger::error!(
                    "tx-pool {name} endpoint unwound; disabled for publisher lifetime"
                );
            })
            .ok()
    })
}

fn bounded_ban_reason(reject: &Reject) -> String {
    bounded_text(format!("reject {reject}"), 1024)
}
fn serialized_recent_reject(reject: &Reject) -> Result<String, Error> {
    let public: PoolTransactionReject = reject.clone().into();
    let mut encoded =
        serde_json::to_string(&public).map_err(|_| Error::Fault("recent rejection encoding"))?;
    if encoded.len() > MAX_TX_POOL_REJECT_DESCRIPTION_BYTES {
        encoded = serde_json::to_string(&PoolTransactionReject::from(Reject::Malformed(
            "tx-pool rejection diagnostic omitted".into(),
            String::new(),
        )))
        .map_err(|_| Error::Fault("recent rejection encoding"))?;
    }
    if encoded.len() > MAX_TX_POOL_REJECT_DESCRIPTION_BYTES {
        return Err(Error::Fault("recent rejection fallback bound"));
    }
    Ok(encoded)
}

const MAX_DYNAMIC_REJECT_TEXT_BYTES: usize = MAX_TX_POOL_REJECT_DESCRIPTION_BYTES - 128;
fn bounded_text(text: String, limit: usize) -> String {
    let boundary = text.floor_char_boundary(text.len().min(limit));
    if boundary == text.len() && text.capacity() <= limit {
        text
    } else {
        text[..boundary].to_owned()
    }
}

fn bound_reject_diagnostic(reject: Reject) -> Reject {
    match reject {
        Reject::Full(message) => Reject::Full(bounded_text(message, MAX_DYNAMIC_REJECT_TEXT_BYTES)),
        Reject::Malformed(kind, message) => {
            let half = MAX_DYNAMIC_REJECT_TEXT_BYTES / 2;
            Reject::Malformed(bounded_text(kind, half), bounded_text(message, half))
        }
        // Never retain a foreign dynamic error graph in the committed
        // journal, even when its Display text happens to be short. Preserve
        // the established public shape by detaching the direct inner
        // diagnostic and letting the typed kind add its prefix exactly once.
        // Detaching `error.to_string()` would duplicate the top-level kind;
        // detaching `root_cause()` would instead erase any meaningful
        // intermediate error context.
        Reject::Verification(error) => {
            let kind = error.kind();
            let detached = match error.cause() {
                Some(cause) => kind.other(bounded_text(
                    cause.to_string(),
                    MAX_DYNAMIC_REJECT_TEXT_BYTES,
                )),
                None => kind.into(),
            };
            Reject::Verification(detached)
        }
        Reject::RBFRejected(message) => {
            Reject::RBFRejected(bounded_text(message, MAX_DYNAMIC_REJECT_TEXT_BYTES))
        }
        Reject::Invalidated(message) => {
            Reject::Invalidated(bounded_text(message, MAX_DYNAMIC_REJECT_TEXT_BYTES))
        }
        Reject::Internal(message) => {
            Reject::Internal(bounded_text(message, MAX_DYNAMIC_REJECT_TEXT_BYTES))
        }
        Reject::Resolve(error) => {
            use ckb_types::core::error::OutPointError;
            Reject::Resolve(match error {
                OutPointError::Dead(point) => OutPointError::Dead(compact_packed(&point)),
                OutPointError::Unknown(point) => OutPointError::Unknown(compact_packed(&point)),
                OutPointError::OutOfOrder(point) => {
                    OutPointError::OutOfOrder(compact_packed(&point))
                }
                OutPointError::InvalidDepGroup(point) => {
                    OutPointError::InvalidDepGroup(compact_packed(&point))
                }
                OutPointError::InvalidHeader(hash) => {
                    OutPointError::InvalidHeader(compact_packed(&hash))
                }
                OutPointError::OverMaxDepExpansionLimit => OutPointError::OverMaxDepExpansionLimit,
            })
        }
        fixed @ (Reject::LowFeeRate(..)
        | Reject::ExceededMaximumAncestorsCount
        | Reject::ExceededTransactionSizeLimit(..)
        | Reject::Duplicated(_)
        | Reject::DeclaredWrongCycles(..)
        | Reject::ExcessiveVerifyTime
        | Reject::Expiry(_)) => fixed,
    }
}
