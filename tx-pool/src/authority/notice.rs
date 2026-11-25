//! Immutable, bounded notices are appended with the owner commit and published
//! serially after its guards open. There is no preallocated sequence gap.
use super::{
    budget::{AccountSnapshot, AccountUsage, Amount, Limits},
    model::{Entry, Error, FullReason, Phase, Source, Status},
    relay::{AuthorityRelaySink, RelayMailboxDisposition, unknown_parents},
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
use ckb_types::{core::BlockView, packed::Byte32, prelude::Unpack};
use ckb_util::parking_lot::{Condvar, Mutex};
use std::{
    collections::{BTreeMap, VecDeque},
    mem::size_of,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

// Bound one endpoint handoff; each batch still settles and releases separately.
const PUBLISH_BATCH_LIMIT: usize = 32;

#[cfg(test)]
#[path = "tests/notice.rs"]
mod tests;

/// Closed domain set of committed post-lock effects.
///
/// Each variant carries only the payloads legal for that outcome; endpoint
/// order is recent rejection, callback, ban, relay, chain observation.
/// These values are fully built before commit and never consult live owners.
#[derive(Clone)]
pub(super) enum Effect {
    Accepted {
        callback: CallbackEvent,
        relay: TxVerificationResult,
    },
    Projected(CallbackEvent),
    Relay(TxVerificationResult),
    Blocks(Vec<Arc<BlockView>>),
    Rejected {
        rejection: Rejection,
        callback: Option<CallbackEvent>,
        relay: Option<TxVerificationResult>,
        log_diagnostic: bool,
    },
    PeerBanned {
        rejection: Rejection,
        peer: PeerIndex,
        deadline: Instant,
        reason: String,
    },
}

/// One bounded reason feeds diagnostics, metrics and optional recent storage.
/// Transient refusals retain their reason through publication without becoming
/// a persistent transaction status. Accepted-victim callbacks own their copy.
#[derive(Clone)]
pub(super) struct Rejection {
    hash: Byte32,
    reason: Reject,
    class: crate::metrics::RejectionClass,
    recent: Option<String>,
    context: Option<Box<RejectionContext>>,
}

/// Only refused candidates allocate this context; successful replacements do
/// not attach it to every victim. No owner or transaction payload is retained.
#[derive(Clone)]
struct RejectionContext {
    stage: &'static str,
    source: &'static str,
    peer: Option<PeerIndex>,
    accounts: [Option<AccountSnapshot>; 5],
}

impl Rejection {
    fn new(hash: &Byte32, reason: Reject) -> Self {
        Self {
            hash: compact_packed(hash),
            class: crate::metrics::RejectionClass::from_reject(&reason),
            reason: bound_reject_diagnostic(reason, MAX_DYNAMIC_REJECT_TEXT_BYTES),
            recent: None,
            context: None,
        }
    }

    fn with_recent(hash: &Byte32, reason: Reject) -> Result<Self, Error> {
        let record = reason.should_recorded();
        let mut rejection = Self::new(hash, reason);
        if record {
            rejection.recent = Some(serialized_recent_reject(&rejection.reason)?);
        }
        Ok(rejection)
    }

    fn log(&self) {
        if !ckb_logger::log_enabled_target!("ckb_tx_pool::rejection", ckb_logger::Level::Debug) {
            return;
        }
        let context = self.context.as_deref();
        let hash: ckb_types::H256 = self.hash.unpack();
        let amount = |amount: Amount| {
            serde_json::json!({
                "items": amount.items, "bytes": amount.bytes, "edges": amount.edges,
                "serialized": amount.serialized, "cycles": amount.cycles,
            })
        };
        let accounts = context
            .into_iter()
            .flat_map(|context| context.accounts.iter().flatten())
            .map(|account| {
                serde_json::json!({
                    "account": account.account,
                    "usage": amount(account.usage), "limit": amount(account.limit),
                })
            })
            .collect::<Vec<_>>();
        // The retained snapshot predates this rejection's commit. It identifies
        // the budget and observed pressure, not the exact refusing reservation.
        ckb_logger::debug_target!(
            "ckb_tx_pool::rejection",
            "{}",
            serde_json::json!({
                "schema": 1, "event": "committed_rejection", "hash": format!("{hash:x}"),
                "reason": format!("{:?}", self.reason),
                "stage": context.map(|context| context.stage).unwrap_or("policy"),
                "source": context.map(|context| context.source),
                "peer": context.and_then(|context| context.peer).map(|peer| peer.value()),
                "resource_observation": "rejection_preparation",
                "accounts": accounts,
            })
        );
    }
}

impl Effect {
    #[cfg(test)]
    pub(super) fn accepted(
        entry: TxEntrySnapshot,
        status: Status,
        peer: Option<PeerIndex>,
    ) -> Self {
        let hash = compact_packed(&entry.transaction.hash());
        Self::accepted_with_hash(entry, hash, status, peer)
    }

    pub(super) fn accepted_with_hash(
        entry: TxEntrySnapshot,
        compact_hash: Byte32,
        status: Status,
        peer: Option<PeerIndex>,
    ) -> Self {
        let callback = match status {
            Status::Proposed => CallbackEvent::Proposed(entry),
            Status::Pending | Status::Gap => CallbackEvent::Pending(entry),
        };
        Self::Accepted {
            callback,
            relay: TxVerificationResult::Ok {
                original_peer: peer,
                tx_hash: compact_hash,
            },
        }
    }

    pub(super) fn projected(entry: TxEntrySnapshot, status: Status) -> Self {
        Self::Projected(match status {
            Status::Proposed => CallbackEvent::Proposed(entry),
            Status::Pending | Status::Gap => CallbackEvent::Pending(entry),
        })
    }

    pub(super) fn relay(result: TxVerificationResult) -> Self {
        Self::Relay(result)
    }

    pub(super) fn waiting(entry: &Entry) -> Option<Self> {
        unknown_parents(entry).map(Self::Relay)
    }

    pub(super) fn blocks(blocks: Vec<Arc<BlockView>>) -> Self {
        Self::Blocks(blocks)
    }

    pub(super) fn banned(
        hash: &Byte32,
        reject: Reject,
        peer: PeerIndex,
        deadline: Instant,
    ) -> Result<Self, Error> {
        let reason = bounded_ban_reason(&reject);
        let mut rejection = Rejection::with_recent(hash, reject)?;
        rejection.context = Some(Box::new(RejectionContext {
            stage: "peer_revocation",
            source: "remote",
            peer: Some(peer),
            accounts: [None; 5],
        }));
        Ok(Self::PeerBanned {
            rejection,
            peer,
            deadline,
            reason,
        })
    }

    pub(super) fn callback(&self) -> Option<&CallbackEvent> {
        match self {
            Self::Accepted { callback, .. } | Self::Projected(callback) => Some(callback),
            Self::Rejected { callback, .. } => callback.as_ref(),
            Self::Relay(_) | Self::Blocks(_) | Self::PeerBanned { .. } => None,
        }
    }

    #[cfg(test)]
    pub(super) fn relay_result(&self) -> Option<&TxVerificationResult> {
        match self {
            Self::PeerBanned { .. } => Some(&TxVerificationResult::GenerationReset),
            _ => self.relay_ref(),
        }
    }

    /// Retire an existing owner. Accepted removals are eligible for relay
    /// rejection; unaccepted removals need a remote origin. Supply a callback
    /// snapshot if and only if the removed owner is accepted.
    pub(super) fn removed(
        entry: &Entry,
        reject: Reject,
        accepted: Option<TxEntrySnapshot>,
    ) -> Result<Self, Error> {
        Self::rejected(
            &entry.hash(),
            reject,
            accepted,
            entry.accepted().is_some() || entry.source.residency_peer().is_some(),
        )
    }

    fn rejected(
        hash: &Byte32,
        reject: Reject,
        accepted: Option<TxEntrySnapshot>,
        relay: bool,
    ) -> Result<Self, Error> {
        // Preserve policy bits before detaching dynamically owned diagnostics.
        let negative =
            relay && reject.is_allowed_relay() && !matches!(reject, Reject::Duplicated(_));
        let log_diagnostic = !(accepted.is_some() && matches!(reject, Reject::RBFRejected(_)));
        let rejection = Rejection::with_recent(hash, reject)?;
        let callback = accepted.map(|entry| CallbackEvent::Reject(entry, rejection.reason.clone()));
        Ok(Self::Rejected {
            rejection,
            callback,
            relay: negative.then(|| TxVerificationResult::Reject {
                tx_hash: compact_packed(hash),
            }),
            log_diagnostic,
        })
    }

    pub(super) fn candidate_rejected(
        hash: &Byte32,
        reject: Reject,
        source: Source,
        before: Option<&Entry>,
        usage: &AccountUsage,
        limits: &Limits,
    ) -> Result<Self, Error> {
        Ok(
            Self::rejected(hash, reject, None, source.residency_peer().is_some())?
                .with_rejection_context(source, before, usage, limits),
        )
    }

    /// Refusing ingress does not decide transaction validity or retire an owner.
    /// Diagnose the pressure and release relay tracking without writing status.
    pub(super) fn capacity_refused(
        hash: &Byte32,
        reason: FullReason,
        source: Source,
        usage: &AccountUsage,
        limits: &Limits,
    ) -> Self {
        Self::Rejected {
            rejection: Rejection::new(hash, Reject::Full(reason.to_string())),
            callback: None,
            relay: source
                .residency_peer()
                .map(|_| TxVerificationResult::Reject {
                    tx_hash: compact_packed(hash),
                }),
            log_diagnostic: true,
        }
        .with_rejection_context(source, None, usage, limits)
    }

    fn with_rejection_context(
        mut self,
        source: Source,
        before: Option<&Entry>,
        usage: &AccountUsage,
        limits: &Limits,
    ) -> Self {
        let peer = source.residency_peer();
        if let Self::Rejected { rejection, .. } = &mut self {
            rejection.context = Some(Box::new(RejectionContext {
                stage: match before.map(|entry| &entry.phase) {
                    None if matches!(source, Source::Local) => "direct_submission",
                    None => "ingress",
                    Some(Phase::Resolve) => "resolve",
                    Some(Phase::Verify(_)) => "verify",
                    Some(Phase::Waiting(_)) => "waiting",
                    Some(Phase::Accepted(_)) => "accepted",
                    Some(Phase::Replaced(_)) => "replaced",
                },
                source: match source {
                    Source::Remote { .. } => "remote",
                    Source::Proposal { .. } => "proposal",
                    Source::Recovery => "recovery",
                    Source::Local => "local",
                },
                peer,
                accounts: if matches!(rejection.reason, Reject::Full(_)) {
                    usage.rejection_snapshot(limits, peer)
                } else {
                    [None; 5]
                },
            }));
        }
        self
    }

    fn rejection(&self) -> Option<&Rejection> {
        match self {
            Self::Rejected { rejection, .. } | Self::PeerBanned { rejection, .. } => {
                Some(rejection)
            }
            _ => None,
        }
    }

    fn relay_ref(&self) -> Option<&TxVerificationResult> {
        match self {
            Self::Accepted { relay, .. } | Self::Relay(relay) => Some(relay),
            Self::Rejected { relay, .. } => relay.as_ref(),
            _ => None,
        }
    }

    fn recent(&self) -> Option<(&Byte32, &str)> {
        let rejection = self.rejection()?;
        rejection
            .recent
            .as_deref()
            .map(|encoded| (&rejection.hash, encoded))
    }

    pub(super) fn reset() -> Self {
        Self::Relay(TxVerificationResult::GenerationReset)
    }

    fn bytes(&self) -> Option<usize> {
        const CALLBACK_METADATA_BYTES: usize =
            size_of::<TxEntrySnapshot>() + MAX_TX_POOL_REJECT_DESCRIPTION_BYTES;
        let mut bytes = size_of::<Self>().checked_add(256)?;
        if let Some(rejection) = self.rejection() {
            bytes = bytes.checked_add(128 + MAX_TX_POOL_REJECT_DESCRIPTION_BYTES)?;
            if let Some(serialized) = &rejection.recent {
                bytes = bytes.checked_add(serialized.capacity())?;
            }
            if rejection.context.is_some() {
                bytes = bytes
                    .checked_add(size_of::<RejectionContext>())?
                    .checked_add(128)?;
            }
        }
        if let Some(event) = self.callback() {
            let entry = match event {
                CallbackEvent::Pending(e)
                | CallbackEvent::Proposed(e)
                | CallbackEvent::Reject(e, _) => e,
            };
            bytes = bytes
                .checked_add(entry.transaction.data().total_size())?
                .checked_add(CALLBACK_METADATA_BYTES)?;
        }
        if let Some(TxVerificationResult::UnknownParents { parents, .. }) = self.relay_ref() {
            bytes = bytes.checked_add(parents.capacity().checked_mul(128)?)?;
        }
        match self {
            Self::Blocks(blocks) => {
                bytes = bytes
                    .checked_add(blocks.capacity().checked_mul(size_of::<Arc<BlockView>>())?)?;
                for block in blocks {
                    bytes = bytes.checked_add(block.data().total_size())?;
                }
            }
            Self::PeerBanned { reason, .. } => {
                bytes = bytes.checked_add(reason.capacity())?;
            }
            _ => {}
        }
        Some(bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NonblockingDrain {
    /// No ready work remains at the FIFO head (queue empty or head not yet ready).
    Idle,
    /// Blocking head, shutdown close, or bounded-prefix spillover needs `TxPoolPublisher`.
    WakeBackground,
    /// Endpoint panicked or FIFO invariant faulted; `publication_failed` already woke waiters.
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    Remote,
    Trusted,
    Critical,
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

#[derive(Clone, Copy)]
struct Quota {
    used: Charge,
    limit: Charge,
}
impl Quota {
    fn new(limit: Charge) -> Self {
        Self {
            used: Charge::default(),
            limit,
        }
    }
}

/// Remote batches also consume ordinary and total capacity; trusted batches
/// consume ordinary and total, leaving the remaining headroom for critical work.
#[derive(Clone, Copy)]
struct NoticeBudget {
    remote: Quota,
    ordinary: Quota,
    total: Quota,
}
impl NoticeBudget {
    fn charged_by(&mut self, class: Class) -> impl Iterator<Item = &mut Quota> {
        let Self {
            remote,
            ordinary,
            total,
        } = self;
        match class {
            Class::Remote => [Some(remote), Some(ordinary), Some(total)],
            Class::Trusted => [None, Some(ordinary), Some(total)],
            Class::Critical => [None, None, Some(total)],
        }
        .into_iter()
        .flatten()
    }
}

/// Bounds one indivisible batch; NoticeBudget accounts for all retained batches.
struct BatchLimit {
    effects: usize,
    bytes: usize,
}

pub(crate) struct Batch {
    effects: Vec<Effect>,
    charge: Charge,
    class: Class,
    ready: AtomicBool,
    published: AtomicBool,
    completed: Notify,
}
impl Batch {
    #[cfg(test)]
    pub(super) fn effects(&self) -> &[Effect] {
        &self.effects
    }

    /// Open publication only after Store::apply has released its commit guards.
    /// An appended batch keeps its FIFO position while waiting for activation.
    pub(super) fn activate(&self, outbox: &Outbox) {
        self.ready.store(true, Ordering::SeqCst);
        outbox.notify_publisher();
    }

    /// Open publication and drain any ready non-blocking FIFO prefix inline on
    /// the calling thread when `publisher` is uncontended.
    ///
    /// Callbacks, `recent_reject` DB writes, peer bans, and block updates stay
    /// isolated on `TxPoolPublisher`. Under concurrent commits, newly ready
    /// non-blocking batches flat-combine onto the active `publisher` holder and
    /// defer behind an unready FIFO head without waking `TxPoolPublisher`.
    pub(super) fn activate_nonblocking(&self, outbox: &Outbox) {
        self.ready.store(true, Ordering::SeqCst);
        outbox.publish_nonblocking_or_defer();
    }

    pub(super) fn is_published(&self) -> bool {
        self.published.load(Ordering::SeqCst)
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
            if self.published.load(Ordering::SeqCst) {
                return Ok(());
            }
            if outbox.faulted.load(Ordering::SeqCst) {
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
    budget: NoticeBudget,
    pending: BTreeMap<Byte32, Weak<Batch>>,
    closed: bool,
}

impl State {
    fn reserve_quota(&mut self, charge: Charge, class: Class) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let mut projected = self.budget;
        for quota in projected.charged_by(class) {
            quota.used = quota
                .used
                .add(charge)
                .filter(|value| value.fits(quota.limit))
                .ok_or(Error::Full(FullReason::NoticeOutbox))?;
        }
        self.budget = projected;
        Ok(())
    }

    fn append_batch(&mut self, batch: &Arc<Batch>) {
        for effect in &batch.effects {
            if let Some((hash, _)) = effect.recent() {
                self.pending
                    .insert(compact_packed(hash), Arc::downgrade(batch));
            }
        }
        self.queue.push_back(Arc::clone(batch));
    }
}
pub(super) struct Outbox {
    state: Mutex<State>,
    // The background task owns this endpoint lifetime. A direct submitter may
    // borrow the same exclusive publisher after its commit guards have opened.
    publisher: Mutex<Option<Endpoints>>,
    // Set when a caller sees `publisher` busy so the current holder wakes the
    // background owner upon release even if the queue looked empty earlier.
    publisher_contended: AtomicBool,
    wake_epoch: AtomicU64,
    publisher_sleeping: AtomicBool,
    wake_lock: Mutex<()>,
    wake_cond: Condvar,
    remote_batch: BatchLimit,
    trusted_batch: BatchLimit,
    critical_batch: BatchLimit,
    faulted: Arc<AtomicBool>,
    pub(super) changed: Notify,
    pub(super) failed: Notify,
    pub(super) room: Notify,
}
#[cfg(test)]
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
                budget: NoticeBudget {
                    remote: Quota::new(remote),
                    ordinary: Quota::new(ordinary),
                    total: Quota::new(total),
                },
                pending: BTreeMap::new(),
                closed: false,
            }),
            publisher: Mutex::new(None),
            publisher_contended: AtomicBool::new(false),
            wake_epoch: AtomicU64::new(0),
            publisher_sleeping: AtomicBool::new(false),
            wake_lock: Mutex::new(()),
            wake_cond: Condvar::new(),
            remote_batch: BatchLimit {
                effects,
                bytes: remote_bytes,
            },
            trusted_batch: BatchLimit {
                effects,
                bytes: admission,
            },
            critical_batch: BatchLimit {
                effects: all_effects,
                bytes: critical,
            },
            faulted,
            changed: Notify::new(),
            failed: Notify::new(),
            room: Notify::new(),
        }))
    }

    fn prepare_batch(
        &self,
        effects: Vec<Effect>,
        class: Class,
    ) -> Result<Option<Arc<Batch>>, Error> {
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
        let batch_limit = match class {
            Class::Remote => &self.remote_batch,
            Class::Trusted => &self.trusted_batch,
            Class::Critical => &self.critical_batch,
        };
        if effects.len() > batch_limit.effects || bytes > batch_limit.bytes {
            return Err(Error::Full("indivisible notice batch".into()));
        }
        let charge = Charge { items: 1, bytes };
        Ok(Some(Arc::new(Batch {
            effects,
            charge,
            class,
            ready: AtomicBool::new(false),
            published: AtomicBool::new(false),
            completed: Notify::new(),
        })))
    }

    /// Reserve quota and append one indivisible batch under a single outbox lock.
    /// The caller must activate the returned batch only after releasing its
    /// authority write lock.
    pub(super) fn commit(
        self: &Arc<Self>,
        effects: Vec<Effect>,
        class: Class,
    ) -> Result<Option<Arc<Batch>>, Error> {
        let Some(batch) = self.prepare_batch(effects, class)? else {
            return Ok(None);
        };
        let mut state = self.state.lock();
        state.reserve_quota(batch.charge, class)?;
        state.append_batch(&batch);
        Ok(Some(batch))
    }

    #[cfg(test)]
    pub(super) fn reserve(
        self: &Arc<Self>,
        effects: Vec<Effect>,
        class: Class,
    ) -> Result<Option<Reservation>, Error> {
        let Some(batch) = self.prepare_batch(effects, class)? else {
            return Ok(None);
        };
        self.state.lock().reserve_quota(batch.charge, class)?;
        Ok(Some(Reservation {
            outbox: Arc::clone(self),
            batch,
            appended: false,
        }))
    }

    pub(super) fn pending_reject(&self, hash: &Byte32) -> Option<String> {
        let batch = self.state.lock().pending.get(hash)?.upgrade()?;
        batch.effects.iter().rev().find_map(|effect| {
            effect
                .recent()
                .filter(|(key, _)| *key == hash)
                .map(|(_, encoded)| encoded.to_owned())
        })
    }

    fn release(&self, batch: &Batch, state: &mut State) -> bool {
        let mut failed = false;
        for quota in state.budget.charged_by(batch.class) {
            if let Some(next) = quota.used.sub(batch.charge) {
                quota.used = next;
            } else {
                self.faulted.store(true, Ordering::SeqCst);
                failed = true;
            }
        }
        failed
    }

    pub(super) fn publish_metrics(&self) {
        let snapshot = {
            let state = self.state.lock();
            let NoticeBudget {
                remote,
                ordinary,
                total,
            } = &state.budget;
            crate::metrics::EffectUsage {
                remote_batches: remote.used.items,
                remote_bytes: remote.used.bytes,
                ordinary_batches: ordinary.used.items,
                ordinary_bytes: ordinary.used.bytes,
                total_batches: total.used.items,
                total_bytes: total.used.bytes,
            }
        };
        snapshot.publish();
    }

    pub(super) fn notify_publisher(&self) {
        self.wake_epoch.fetch_add(1, Ordering::SeqCst);
        if self.publisher_sleeping.swap(false, Ordering::SeqCst) {
            let _guard = self.wake_lock.lock();
            self.wake_cond.notify_all();
        }
        self.changed.notify_one();
    }

    pub(super) fn close(&self) {
        self.state.lock().closed = true;
        self.notify_publisher();
        self.room.notify_waiters();
    }

    pub(super) fn drained(&self) -> bool {
        let state = self.state.lock();
        state.closed && state.queue.is_empty() && state.budget.total.used.items == 0
    }

    #[cfg(test)]
    pub(super) fn idle_for_test(&self) -> bool {
        let state = self.state.lock();
        let budget = &state.budget;
        state.queue.is_empty()
            && state.pending.is_empty()
            && [budget.remote.used, budget.ordinary.used, budget.total.used]
                .into_iter()
                .all(|charge| charge.items == 0 && charge.bytes == 0)
    }

    #[cfg(test)]
    pub(super) fn drain_for_test(&self) {
        let mut state = self.state.lock();
        while let Some(batch) = state.queue.pop_front() {
            self.release(&batch, &mut state);
        }
        state.pending.clear();
    }

    #[cfg(test)]
    fn publish_ready(&self, endpoints: &mut Endpoints) -> Result<bool, Error> {
        Ok(self.publish_ready_step(endpoints)?.0)
    }

    fn publish_ready_step(&self, endpoints: &mut Endpoints) -> Result<(bool, bool), Error> {
        let (head, count) = {
            let state = self.state.lock();
            let Some(head) = state
                .queue
                .front()
                .filter(|batch| batch.ready.load(Ordering::SeqCst))
            else {
                return Ok((false, false));
            };
            let count = state
                .queue
                .iter()
                .take(PUBLISH_BATCH_LIMIT)
                .take_while(|batch| batch.ready.load(Ordering::SeqCst))
                .count();
            (Arc::clone(head), count)
        };
        let offload = count > 1
            && head
                .effects
                .iter()
                .filter_map(Effect::callback)
                .any(|event| endpoints.callback_enabled(event));
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
        let has_more = if offload {
            #[cfg(feature = "profiling")]
            let _offload_span = tracing::trace_span!(
                target: "ckb_tx_pool_profile", "tx_pool.publisher.offload"
            )
            .entered();
            block_offload(|| self.publish_prefix(endpoints, head, count))?
        } else {
            self.publish_prefix(endpoints, head, count)?
        };
        Ok((true, has_more))
    }

    fn retire_published_head(
        &self,
        batch: &Arc<Batch>,
        state: &mut State,
    ) -> Result<(Option<Arc<Batch>>, bool), Error> {
        if !state
            .queue
            .front()
            .is_some_and(|head| Arc::ptr_eq(head, batch))
        {
            return Err(Error::Fault("notice FIFO head"));
        }
        for effect in &batch.effects {
            if let Some((hash, _)) = effect.recent()
                && state
                    .pending
                    .get(hash)
                    .is_some_and(|old| std::ptr::eq(old.as_ptr(), Arc::as_ptr(batch)))
            {
                state.pending.remove(hash);
            }
        }
        let retired = state.queue.pop_front();
        let failed = self.release(batch, state);
        Ok((retired, failed))
    }

    fn complete_published_batch(
        &self,
        batch: &Arc<Batch>,
        retired: Option<Arc<Batch>>,
        failed: bool,
    ) {
        batch.published.store(true, Ordering::SeqCst);
        batch.completed.notify_waiters();
        if failed {
            self.failed.notify_waiters();
        }
        self.room.notify_waiters();
        drop(retired);
    }

    /// Settle a ready FIFO prefix without await. Complete each batch before
    /// running the next endpoint; displaced payloads outlive the state lock.
    /// Returns `true` if remaining batches or shutdown close need the background owner.
    fn publish_prefix(
        &self,
        endpoints: &mut Endpoints,
        head: Arc<Batch>,
        count: usize,
    ) -> Result<bool, Error> {
        // One publisher owns removal, and readiness only opens.
        let mut next = Some(head);
        let mut wake_background = false;
        for remaining in (0..count).rev() {
            let batch = next.take().ok_or(Error::Fault("notice FIFO head"))?;
            // No await in a batch terminal: cancellation cannot replay a prefix.
            endpoints.publish_batch(&batch.effects);
            let mut state = self.state.lock();
            let (retired, failed) = self.retire_published_head(&batch, &mut state)?;
            if remaining > 0 {
                next = state
                    .queue
                    .front()
                    .filter(|batch| batch.ready.load(Ordering::SeqCst))
                    .cloned();
            } else {
                wake_background = state.closed || !state.queue.is_empty();
            }
            drop(state);
            self.complete_published_batch(&batch, retired, failed);
        }
        Ok(wake_background)
    }

    fn publication_failed(&self) {
        crate::metrics::record_failure(crate::metrics::FailureBoundary::EffectPublisher);
        self.faulted.store(true, Ordering::SeqCst);
        self.failed.notify_waiters();
        self.room.notify_waiters();
        self.notify_publisher();
    }

    /// Borrow the sole publisher without waiting for another endpoint pass.
    /// The FIFO prefix and endpoint failure state remain under one exclusive
    /// lease, including across synchronous callbacks and their nested reads.
    pub(super) fn try_publish(&self) -> Result<bool, Error> {
        let mut publisher = match self.publisher.try_lock() {
            Some(guard) => guard,
            None => {
                // Publish contention before one retry so a concurrent holder
                // releasing `publisher` cannot miss our wakeup flag (SeqCst Dekker).
                self.publisher_contended.store(true, Ordering::SeqCst);
                match self.publisher.try_lock() {
                    Some(guard) => guard,
                    None => return Ok(false),
                }
            }
        };
        let Some(endpoints) = publisher.as_mut() else {
            return Ok(false);
        };
        struct Completion<'a> {
            outbox: &'a Outbox,
            done: bool,
        }
        impl Drop for Completion<'_> {
            fn drop(&mut self) {
                if !self.done {
                    self.outbox.publication_failed();
                }
            }
        }
        let mut completion = Completion {
            outbox: self,
            done: false,
        };
        let (progress, wake_background) = self.publish_ready_step(endpoints)?;
        completion.done = true;
        drop(publisher);
        if (progress && wake_background) || self.publisher_contended.swap(false, Ordering::SeqCst) {
            // Wake the background owner only when more batches remain, shutdown
            // is draining, or another caller observed the publisher busy.
            self.notify_publisher();
        }
        Ok(progress)
    }

    /// Drain a bounded leading ready non-blocking FIFO prefix under the held
    /// `publisher` lease. Stops immediately before any blocking batch, unready
    /// head, shutdown close, or `PUBLISH_BATCH_LIMIT`.
    fn drain_nonblocking_prefix(&self, endpoints: &mut Endpoints) -> NonblockingDrain {
        let mut next = {
            let state = self.state.lock();
            if state.closed {
                return NonblockingDrain::WakeBackground;
            }
            let Some(head) = state.queue.front() else {
                return NonblockingDrain::Idle;
            };
            if !head.ready.load(Ordering::SeqCst) {
                return NonblockingDrain::Idle;
            }
            if !endpoints.batch_is_nonblocking(head) {
                return NonblockingDrain::WakeBackground;
            }
            Arc::clone(head)
        };
        struct Completion<'a> {
            outbox: &'a Outbox,
            done: bool,
        }
        impl Drop for Completion<'_> {
            fn drop(&mut self) {
                if !self.done {
                    self.outbox.publication_failed();
                }
            }
        }
        let mut completion = Completion {
            outbox: self,
            done: false,
        };
        let mut wake_background = false;
        for drained in 1..=PUBLISH_BATCH_LIMIT {
            let batch = next;
            endpoints.publish_batch(&batch.effects);
            let mut state = self.state.lock();
            let Ok((retired, failed)) = self.retire_published_head(&batch, &mut state) else {
                return NonblockingDrain::Faulted;
            };
            let following = if state.closed {
                wake_background = true;
                None
            } else if let Some(head) = state.queue.front() {
                if !head.ready.load(Ordering::SeqCst) {
                    None
                } else if drained < PUBLISH_BATCH_LIMIT && endpoints.batch_is_nonblocking(head) {
                    Some(Arc::clone(head))
                } else {
                    wake_background = true;
                    None
                }
            } else {
                None
            };
            drop(state);
            self.complete_published_batch(&batch, retired, failed);
            if let Some(following) = following {
                next = following;
            } else {
                break;
            }
        }
        completion.done = true;
        if wake_background {
            NonblockingDrain::WakeBackground
        } else {
            NonblockingDrain::Idle
        }
    }

    /// Borrow the exclusive publisher without blocking and drain only a leading
    /// ready FIFO prefix whose effects require no enabled external callbacks,
    /// no persistent `recent_reject` DB writes, no peer bans, and no block
    /// updates. Flat-combines contended non-blocking batches onto the active
    /// `publisher` holder and avoids waking `TxPoolPublisher` unless a blocking
    /// batch, shutdown drain, or bounded-pass spillover remains.
    pub(super) fn publish_nonblocking_or_defer(&self) {
        for attempt in 0..2 {
            let mut publisher = match self.publisher.try_lock() {
                Some(guard) => guard,
                None => {
                    if attempt == 0 {
                        // SeqCst Dekker: publish contention before one final
                        // try_lock so the current holder cannot release
                        // `publisher` without observing our ready batch.
                        self.publisher_contended.store(true, Ordering::SeqCst);
                        match self.publisher.try_lock() {
                            Some(guard) => guard,
                            None => return,
                        }
                    } else {
                        self.notify_publisher();
                        return;
                    }
                }
            };
            let Some(endpoints) = publisher.as_mut() else {
                drop(publisher);
                self.notify_publisher();
                return;
            };
            // Clear any pre-existing contention flag before scanning `state.queue`
            // so batches activated prior to this scan are drained in one pass,
            // while any caller contending after this point sets the flag again
            // before our post-drop `swap(false)` check.
            self.publisher_contended.store(false, Ordering::SeqCst);
            let outcome = self.drain_nonblocking_prefix(endpoints);
            drop(publisher);
            match outcome {
                NonblockingDrain::WakeBackground => {
                    self.notify_publisher();
                    return;
                }
                NonblockingDrain::Idle => {
                    if !self.publisher_contended.swap(false, Ordering::SeqCst) {
                        return;
                    }
                }
                NonblockingDrain::Faulted => return,
            }
        }
        self.notify_publisher();
    }

    /// Run the sole publisher on one dedicated OS thread so synchronous
    /// endpoints (callbacks, `recent_reject`, `fee_estimator`, `ban_peer`)
    /// never invoke Tokio `block_in_place` or churn `GlobalRt` threads.
    pub(super) fn spawn_dedicated(
        self: Arc<Self>,
        handle: &ckb_async_runtime::Handle,
        endpoints: Endpoints,
        signal: CancellationToken,
    ) -> tokio::task::JoinHandle<Result<(), Error>> {
        {
            let mut publisher = self.publisher.lock();
            if publisher.is_some() {
                return handle.spawn(async { Err(Error::Fault("duplicate notice publisher")) });
            }
            *publisher = Some(endpoints);
        }
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let worker_outbox = Arc::clone(&self);
        let worker_signal = signal.clone();
        let spawned = std::thread::Builder::new()
            .name("TxPoolPublisher".to_owned())
            .spawn(move || {
                struct Completion {
                    outbox: Arc<Outbox>,
                    signal: CancellationToken,
                    done: bool,
                }
                impl Drop for Completion {
                    fn drop(&mut self) {
                        if !self.done {
                            self.signal.cancel();
                        }
                        let endpoints = self.outbox.publisher.lock().take();
                        drop(endpoints);
                        if !self.done {
                            self.outbox.publication_failed();
                        }
                    }
                }
                let mut completion = Completion {
                    outbox: Arc::clone(&worker_outbox),
                    signal: worker_signal,
                    done: false,
                };
                let result = loop {
                    let observed = worker_outbox.wake_epoch.load(Ordering::SeqCst);
                    match worker_outbox.try_publish() {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(error) => break Err(error),
                    }
                    if worker_outbox.drained() {
                        completion.done = true;
                        break Ok(());
                    }
                    if worker_outbox.faulted.load(Ordering::SeqCst) {
                        break Err(Error::Fault("notice publisher"));
                    }
                    let mut guard = worker_outbox.wake_lock.lock();
                    worker_outbox
                        .publisher_sleeping
                        .store(true, Ordering::SeqCst);
                    if worker_outbox.wake_epoch.load(Ordering::SeqCst) == observed
                        && !worker_outbox.drained()
                        && !worker_outbox.faulted.load(Ordering::SeqCst)
                    {
                        worker_outbox.wake_cond.wait(&mut guard);
                    }
                    worker_outbox
                        .publisher_sleeping
                        .store(false, Ordering::SeqCst);
                };
                drop(completion);
                let _ = done_tx.send(result);
            });
        match spawned {
            Ok(thread_handle) => {
                let abort_outbox = Arc::clone(&self);
                handle.spawn(async move {
                    struct AbortGuard {
                        outbox: Arc<Outbox>,
                        signal: CancellationToken,
                        thread: Option<std::thread::JoinHandle<()>>,
                        finished: bool,
                    }
                    impl Drop for AbortGuard {
                        fn drop(&mut self) {
                            if !self.finished {
                                self.signal.cancel();
                                self.outbox.publication_failed();
                            }
                            if let Some(thread) = self.thread.take() {
                                let _ = thread.join();
                            }
                        }
                    }
                    let mut guard = AbortGuard {
                        outbox: abort_outbox,
                        signal,
                        thread: Some(thread_handle),
                        finished: false,
                    };
                    let result = match done_rx.await {
                        Ok(result) => result,
                        Err(_) => Err(Error::Fault("notice publisher")),
                    };
                    guard.finished = true;
                    if let Some(thread) = guard.thread.take() {
                        let _ = tokio::task::spawn_blocking(move || {
                            let _ = thread.join();
                        })
                        .await;
                    }
                    result
                })
            }
            Err(_) => {
                let endpoints = self.publisher.lock().take();
                handle.spawn(async move {
                    let Some(endpoints) = endpoints else {
                        return Err(Error::Fault("notice publisher"));
                    };
                    self.run(endpoints, signal).await
                })
            }
        }
    }

    pub(super) async fn run(
        self: Arc<Self>,
        endpoints: Endpoints,
        signal: CancellationToken,
    ) -> Result<(), Error> {
        {
            let mut publisher = self.publisher.lock();
            if publisher.is_some() {
                return Err(Error::Fault("duplicate notice publisher"));
            }
            *publisher = Some(endpoints);
        }
        struct Completion {
            outbox: Arc<Outbox>,
            signal: CancellationToken,
            done: bool,
        }
        impl Drop for Completion {
            fn drop(&mut self) {
                // Runtime shutdown may drop the publisher before any request
                // supervisor. Close their intake before joining a callback
                // which can itself be waiting for a reserved read.
                if !self.done {
                    self.signal.cancel();
                }
                // Cancellation cannot release the owner while a submitter is
                // inside a callback. Once the lease returns, retire endpoints
                // before the background task reports completion.
                let endpoints = if let Some(mut publisher) = self.outbox.publisher.try_lock() {
                    publisher.take()
                } else {
                    block_offload(|| self.outbox.publisher.lock().take())
                };
                drop(endpoints);
                if !self.done {
                    self.outbox.publication_failed();
                }
            }
        }
        let mut completion = Completion {
            outbox: Arc::clone(&self),
            signal,
            done: false,
        };
        loop {
            let changed = self.changed.notified();
            if self.try_publish()? {
                continue;
            }
            if self.drained() {
                completion.done = true;
                return Ok(());
            }
            if self.faulted.load(Ordering::SeqCst) {
                return Err(Error::Fault("notice publisher"));
            }
            changed.await;
        }
    }
}
#[cfg(test)]
impl Reservation {
    /// Called only after the final owner cut has no expected failure left.
    pub(super) fn append(mut self) -> Arc<Batch> {
        let batch = Arc::clone(&self.batch);
        self.appended = true;
        self.outbox.state.lock().append_batch(&batch);
        batch
    }
}
#[cfg(test)]
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
            self.outbox.notify_publisher();
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
        !self.callbacks_disabled && self.callbacks.has_callback(event)
    }

    fn batch_is_nonblocking(&self, batch: &Batch) -> bool {
        batch.effects.iter().all(|effect| match effect {
            Effect::Accepted { callback, .. } | Effect::Projected(callback) => {
                !self.callback_enabled(callback)
            }
            Effect::Relay(_) => true,
            Effect::Rejected {
                rejection,
                callback,
                ..
            } => {
                callback
                    .as_ref()
                    .is_none_or(|event| !self.callback_enabled(event))
                    && (self.recent.is_none() || rejection.recent.is_none())
            }
            Effect::Blocks(_) | Effect::PeerBanned { .. } => false,
        })
    }

    fn publish_callback(&mut self, event: &CallbackEvent) {
        if self.callback_enabled(event)
            && run_endpoint("callbacks", || {
                crate::callback::with_callback_context(|| self.callbacks.publish(event))
            })
            .is_none()
        {
            self.callbacks_disabled = true;
        }
    }

    fn publish_relay(&mut self, result: &TxVerificationResult) {
        if !self.relay_disabled
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
    }

    fn publish_rejection(
        &mut self,
        rejection: &Rejection,
        log_diagnostic: bool,
        write_recent: bool,
    ) {
        rejection.class.record();
        if log_diagnostic {
            rejection.log();
        }
        if write_recent
            && let (Some(store), Some(serialized)) = (&self.recent, rejection.recent.as_deref())
        {
            self.recent_writes
                .write(|| store.put_serialized(&rejection.hash, serialized));
        }
    }

    #[cfg(test)]
    fn publish(&mut self, effect: &Effect) {
        self.publish_effect(effect, true);
    }

    fn publish_batch(&mut self, effects: &[Effect]) {
        let mut batched_recent = false;
        if let Some(store) = &self.recent {
            let mut recent_count = 0usize;
            for effect in effects {
                if effect.recent().is_some() {
                    recent_count = recent_count.saturating_add(1);
                    if recent_count >= 2 {
                        break;
                    }
                }
            }
            if recent_count >= 2 {
                let pairs: Vec<(&Byte32, &str)> =
                    effects.iter().filter_map(Effect::recent).collect();
                self.recent_writes
                    .write(|| store.put_serialized_batch(&pairs));
                batched_recent = true;
            }
        }
        for effect in effects {
            self.publish_effect(effect, !batched_recent);
        }
    }

    fn publish_effect(&mut self, effect: &Effect, write_recent: bool) {
        #[cfg(feature = "profiling")]
        let _span = tracing::trace_span!(target: "ckb_tx_pool_profile", "tx_pool.effects.publish")
            .entered();
        match effect {
            Effect::Accepted { callback, relay } => {
                self.publish_callback(callback);
                self.publish_relay(relay);
            }
            Effect::Projected(callback) => {
                self.publish_callback(callback);
            }
            Effect::Relay(result) => {
                self.publish_relay(result);
            }
            Effect::Blocks(blocks) => {
                if !self.estimator_disabled {
                    for block in blocks {
                        if run_endpoint("fee_estimator", || self.estimator.commit_block(block))
                            .is_none()
                        {
                            self.estimator_disabled = true;
                            break;
                        }
                    }
                }
            }
            Effect::Rejected {
                rejection,
                callback,
                relay,
                log_diagnostic,
            } => {
                self.publish_rejection(rejection, *log_diagnostic, write_recent);
                if let Some(event) = callback {
                    self.publish_callback(event);
                }
                if let Some(result) = relay {
                    self.publish_relay(result);
                }
            }
            Effect::PeerBanned {
                rejection,
                peer,
                deadline,
                reason,
            } => {
                self.publish_rejection(rejection, true, write_recent);
                if !self.ban_disabled
                    && run_endpoint("network_ban", || {
                        if let Some(duration) = deadline.checked_duration_since(Instant::now()) {
                            self.network.ban_peer(*peer, duration, reason.clone());
                        }
                    })
                    .is_none()
                {
                    self.ban_disabled = true;
                }
                self.publish_relay(&TxVerificationResult::GenerationReset);
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
    let encode = |reject| {
        let public = PoolTransactionReject::try_from(reject)
            .map_err(|_| Error::Fault("internal rejection cannot be recorded"))?;
        serde_json::to_string(&public).map_err(|_| Error::Fault("recent rejection encoding"))
    };
    let mut encoded = encode(reject.clone())?;
    if encoded.len() > MAX_TX_POOL_REJECT_DESCRIPTION_BYTES {
        // A control byte can expand to six JSON bytes. Bound the description
        // more tightly without changing the rejection's public type.
        let bounded = bound_reject_diagnostic(reject.clone(), MAX_DYNAMIC_REJECT_TEXT_BYTES / 6);
        encoded = encode(bounded)?;
    }
    if encoded.len() > MAX_TX_POOL_REJECT_DESCRIPTION_BYTES {
        return Err(Error::Fault("recent rejection encoding bound"));
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

fn bound_reject_diagnostic(reject: Reject, limit: usize) -> Reject {
    match reject {
        Reject::Full(message) => Reject::Full(bounded_text(message, limit)),
        Reject::Malformed(kind, message) => {
            let half = limit / 2;
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
            let detached = match error.downcast_ref::<ckb_error::InternalError>() {
                // Keep interruption distinguishable from a transaction verdict
                // after detaching the diagnostic's owned error graph.
                Some(internal) => {
                    let detached = match internal.cause() {
                        Some(cause) => internal
                            .kind()
                            .other(bounded_text(cause.to_string(), limit)),
                        None => internal.kind().into(),
                    };
                    kind.because(detached)
                }
                None => match error.cause() {
                    Some(cause) => kind.other(bounded_text(cause.to_string(), limit)),
                    None => kind.into(),
                },
            };
            Reject::Verification(detached)
        }
        Reject::RBFRejected(message) => Reject::RBFRejected(bounded_text(message, limit)),
        Reject::Invalidated(message) => Reject::Invalidated(bounded_text(message, limit)),
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
