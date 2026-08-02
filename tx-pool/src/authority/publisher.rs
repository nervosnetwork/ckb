//! Exhaustive post-commit publication for the unified authority.
//!
//! The compiler in this module consumes only [`CommittedEffect`] values. It
//! never rereads transaction authority after Apply, so a later admission,
//! reorg, or replacement cannot change the meaning of an older outcome. The
//! publisher owns one move-only effect lease at a time; cancellation returns
//! the complete lease to the authority head before the task disappears.

use super::{
    effect::{
        CommittedAcceptance, CommittedConflictOwner, CommittedEffect, CommittedEntrySnapshot,
        CommittedRejection, EffectEndpoint, EffectLease, EffectProgressError, RejectionAudience,
    },
    plan::{EffectSettlementFailure, PlanError},
    runtime::AuthorityRuntime,
    state::{AcceptedStatus, RawTxHash},
};
use crate::{
    callback::{CallbackEvent, Callbacks},
    component::{entry::TxEntrySnapshot, recent_reject::RecentReject},
    constants::MALFORMED_TX_BAN_SECONDS,
    error::Reject,
    network::TxPoolNetworkHandle,
    service::{
        TxVerificationResult,
        effects::{bounded_commit_ban_reason, serialized_recent_reject},
    },
    util::compact_packed,
};
use ckb_channel::TrySendError;
use ckb_logger::{error, info};
use ckb_network::PeerIndex;
use ckb_types::{core::error::OutPointError, packed::Byte32};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const EXTERNAL_EFFECT_TIMEOUT: Duration = Duration::from_secs(1);
const RELAY_RETRY_TIMEOUT: Duration = Duration::from_millis(250);
const RELAY_RETRY_DELAY: Duration = Duration::from_millis(1);

#[derive(Debug)]
pub(crate) struct AuthorityEffectPublisherFault {
    pub(super) kind: AuthorityEffectPublisherFaultKind,
}

#[derive(Debug)]
pub(super) enum AuthorityEffectPublisherFaultKind {
    ConcurrentConsumer,
    Authority(PlanError),
    Settlement(EffectSettlementFailure),
    Progress(EffectProgressError),
    RelayDisconnected,
}

impl AuthorityEffectPublisherFault {
    fn concurrent_consumer() -> Self {
        Self {
            kind: AuthorityEffectPublisherFaultKind::ConcurrentConsumer,
        }
    }

    fn authority(error: PlanError) -> Self {
        Self {
            kind: AuthorityEffectPublisherFaultKind::Authority(error),
        }
    }

    fn settlement(failure: EffectSettlementFailure) -> Self {
        Self {
            kind: AuthorityEffectPublisherFaultKind::Settlement(failure),
        }
    }

    fn progress(error: EffectProgressError) -> Self {
        Self {
            kind: AuthorityEffectPublisherFaultKind::Progress(error),
        }
    }

    fn relay_disconnected() -> Self {
        Self {
            kind: AuthorityEffectPublisherFaultKind::RelayDisconnected,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityEffectEndpointConfigError {
    CallbackWorker,
}

struct CallbackJob {
    callbacks: Arc<Callbacks>,
    event: CallbackEvent,
    done: tokio::sync::oneshot::Sender<()>,
}

/// Stable foreign endpoints and their one-way circuit breakers.
///
/// Circuits are operational projections, not transaction state. Opening one
/// may discard only that endpoint's observational detail; it cannot reverse a
/// committed transition or suppress the required relay channel.
pub(crate) struct AuthorityEffectEndpoints {
    network: TxPoolNetworkHandle,
    tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
    callbacks: Arc<Callbacks>,
    recent_reject: Option<Arc<RecentReject>>,
    callback_sender: std::sync::mpsc::SyncSender<CallbackJob>,
    callback_circuit_open: AtomicBool,
    network_circuit_open: AtomicBool,
    recent_reject_circuit_open: AtomicBool,
}

impl AuthorityEffectEndpoints {
    pub(crate) fn new(
        network: TxPoolNetworkHandle,
        tx_relay_sender: ckb_channel::Sender<TxVerificationResult>,
        callbacks: Arc<Callbacks>,
        recent_reject: Option<Arc<RecentReject>>,
    ) -> Result<Self, AuthorityEffectEndpointConfigError> {
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
            .map_err(|_| AuthorityEffectEndpointConfigError::CallbackWorker)?;
        Ok(Self {
            network,
            tx_relay_sender,
            callbacks,
            recent_reject,
            callback_sender,
            callback_circuit_open: AtomicBool::new(false),
            network_circuit_open: AtomicBool::new(false),
            recent_reject_circuit_open: AtomicBool::new(false),
        })
    }

    async fn publish_endpoint(
        &self,
        outcome: &mut CompiledEndpointOutcome,
        endpoint: EffectEndpoint,
        relay_reconciled: &mut bool,
    ) -> Result<EndpointDisposition, AuthorityEffectPublisherFault> {
        match endpoint {
            EffectEndpoint::RecentReject => Ok(match outcome.recent_reject.take() {
                Some(recent) => self.publish_recent_reject(recent).await,
                None => EndpointDisposition::Published,
            }),
            EffectEndpoint::Callback => Ok(match outcome.callback.take() {
                Some(callback) => self.publish_callback(callback).await,
                None => EndpointDisposition::Published,
            }),
            EffectEndpoint::Ban => Ok(match outcome.ban.take() {
                Some(ban) => self.publish_ban(ban).await,
                None => EndpointDisposition::Published,
            }),
            EffectEndpoint::Relay => {
                let Some(relay) = outcome.relay.take() else {
                    return Ok(EndpointDisposition::Published);
                };
                if (!*relay_reconciled || relay.is_required())
                    && self.publish_relay(relay).await? == RelayDisposition::Reconciled
                {
                    *relay_reconciled = true;
                }
                Ok(EndpointDisposition::Published)
            }
        }
    }

    #[cfg(test)]
    pub(super) async fn publish(
        &self,
        mut outcome: CompiledEndpointOutcome,
        relay_reconciled: &mut bool,
    ) -> Result<EndpointDisposition, AuthorityEffectPublisherFault> {
        let mut disposition = EndpointDisposition::Published;
        for endpoint in EffectEndpoint::ORDER {
            disposition = disposition.join(
                self.publish_endpoint(&mut outcome, endpoint, relay_reconciled)
                    .await?,
            );
        }
        Ok(disposition)
    }

    async fn publish_callback(&self, event: CallbackEvent) -> EndpointDisposition {
        let registered = match &event {
            CallbackEvent::Pending(_) => self.callbacks.pending.is_some(),
            CallbackEvent::Proposed(_) => self.callbacks.proposed.is_some(),
            CallbackEvent::Reject(_, _) => self.callbacks.reject.is_some(),
        };
        if !registered {
            return EndpointDisposition::Published;
        }
        if self.callback_circuit_open.load(Ordering::Acquire) {
            return EndpointDisposition::CircuitDisposed;
        }
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        if let Err(send_error) = self.callback_sender.try_send(CallbackJob {
            callbacks: Arc::clone(&self.callbacks),
            event,
            done: done_tx,
        }) {
            self.callback_circuit_open.store(true, Ordering::Release);
            error!("tx-pool callback endpoint unavailable: {send_error}");
            return EndpointDisposition::CircuitDisposed;
        }
        if matches!(
            tokio::time::timeout(EXTERNAL_EFFECT_TIMEOUT, done_rx).await,
            Ok(Ok(()))
        ) {
            EndpointDisposition::Published
        } else {
            self.callback_circuit_open.store(true, Ordering::Release);
            error!("tx-pool callback timed out; callback circuit opened");
            EndpointDisposition::CircuitDisposed
        }
    }

    async fn publish_ban(&self, ban: BanAction) -> EndpointDisposition {
        if self.network_circuit_open.load(Ordering::Acquire) {
            return EndpointDisposition::CircuitDisposed;
        }
        let network = Arc::clone(&self.network);
        let published = run_blocking_effect(move || {
            network.ban_peer(ban.peer, ban.duration, ban.reason);
        })
        .await;
        if let Err(failure) = published {
            self.network_circuit_open.store(true, Ordering::Release);
            error!("tx-pool network effect failed; circuit opened: {failure}");
            EndpointDisposition::CircuitDisposed
        } else {
            EndpointDisposition::Published
        }
    }

    async fn publish_recent_reject(&self, recent: RecentRejectAction) -> EndpointDisposition {
        let Some(store) = self.recent_reject.as_ref().map(Arc::clone) else {
            return EndpointDisposition::Published;
        };
        if self.recent_reject_circuit_open.load(Ordering::Acquire) {
            return EndpointDisposition::CircuitDisposed;
        }
        let serialized = match serialized_recent_reject(&recent.reject) {
            Ok(serialized) => serialized,
            Err(encoding_error) => {
                self.recent_reject_circuit_open
                    .store(true, Ordering::Release);
                error!("failed to encode bounded recent reject: {encoding_error}");
                return EndpointDisposition::CircuitDisposed;
            }
        };
        let published = run_blocking_effect(move || {
            store
                .put_serialized(&recent.tx_hash, &serialized)
                .map_err(|store_error| {
                    format!(
                        "failed to record recent reject {}: {store_error}",
                        recent.tx_hash
                    )
                })
        })
        .await;
        match published {
            Ok(Ok(())) => EndpointDisposition::Published,
            Ok(Err(store_error)) => {
                self.recent_reject_circuit_open
                    .store(true, Ordering::Release);
                error!("{store_error}");
                EndpointDisposition::CircuitDisposed
            }
            Err(failure) => {
                self.recent_reject_circuit_open
                    .store(true, Ordering::Release);
                error!("tx-pool recent-reject effect failed; circuit opened: {failure}");
                EndpointDisposition::CircuitDisposed
            }
        }
    }

    pub(super) async fn publish_relay(
        &self,
        action: RelayAction,
    ) -> Result<RelayDisposition, AuthorityEffectPublisherFault> {
        let required = action.is_required();
        let started = tokio::time::Instant::now();
        let mut pending = action.result;
        let mut reconciled = false;
        loop {
            match self.tx_relay_sender.try_send(pending) {
                Ok(()) => {
                    return Ok(if reconciled {
                        RelayDisposition::Reconciled
                    } else {
                        RelayDisposition::Exact
                    });
                }
                Err(TrySendError::Full(returned)) => {
                    if !required && !reconciled && started.elapsed() >= RELAY_RETRY_TIMEOUT {
                        pending = TxVerificationResult::GenerationReset;
                        reconciled = true;
                        error!(
                            "tx-pool relayer endpoint remained full; replacing detail with GenerationReset"
                        );
                    } else {
                        pending = returned;
                    }
                    tokio::time::sleep(RELAY_RETRY_DELAY).await;
                }
                Err(TrySendError::Disconnected(_)) => {
                    return Err(AuthorityEffectPublisherFault::relay_disconnected());
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EndpointDisposition {
    Published,
    CircuitDisposed,
}

impl EndpointDisposition {
    fn join(self, other: Self) -> Self {
        if matches!(self, Self::CircuitDisposed) || matches!(other, Self::CircuitDisposed) {
            Self::CircuitDisposed
        } else {
            Self::Published
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelayDisposition {
    Exact,
    Reconciled,
}

pub(super) struct RelayAction {
    pub(super) result: TxVerificationResult,
    required: bool,
}

impl RelayAction {
    pub(super) fn ordinary(result: TxVerificationResult) -> Self {
        Self {
            result,
            required: false,
        }
    }

    pub(super) fn required(result: TxVerificationResult) -> Self {
        Self {
            result,
            required: true,
        }
    }

    pub(super) const fn is_required(&self) -> bool {
        self.required
    }
}

pub(super) struct RecentRejectAction {
    pub(super) tx_hash: Byte32,
    pub(super) reject: Reject,
}

pub(super) struct BanAction {
    pub(super) peer: PeerIndex,
    pub(super) duration: Duration,
    pub(super) reason: String,
}

#[derive(Default)]
pub(super) struct CompiledEndpointOutcome {
    pub(super) recent_reject: Option<RecentRejectAction>,
    pub(super) callback: Option<CallbackEvent>,
    pub(super) ban: Option<BanAction>,
    pub(super) relay: Option<RelayAction>,
}

pub(super) fn compile_committed_effect(effect: CommittedEffect) -> CompiledEndpointOutcome {
    match effect {
        CommittedEffect::Accepted(acceptance) => compile_acceptance(acceptance),
        CommittedEffect::Rejected(rejection) => compile_rejection(rejection),
        CommittedEffect::ChainCommitted {
            tx_hash,
            ingress_peer,
        } => CompiledEndpointOutcome {
            relay: Some(RelayAction::ordinary(TxVerificationResult::Ok {
                original_peer: Some(ingress_peer),
                tx_hash: compact_hash(&tx_hash),
            })),
            ..Default::default()
        },
        CommittedEffect::PeerRevoked { tx_hash, .. }
        | CommittedEffect::RemoteExpired { tx_hash, .. } => CompiledEndpointOutcome {
            relay: Some(RelayAction::ordinary(TxVerificationResult::Reject {
                tx_hash: compact_hash(&tx_hash),
            })),
            ..Default::default()
        },
        CommittedEffect::ParentTransactionsRequested(request) => {
            let parents = request
                .parents()
                .iter()
                .map(compact_hash)
                .collect::<HashSet<_>>();
            CompiledEndpointOutcome {
                relay: Some(RelayAction::required(
                    TxVerificationResult::UnknownParents {
                        peer: request.peer(),
                        parents,
                    },
                )),
                ..Default::default()
            }
        }
        CommittedEffect::GenerationReset => CompiledEndpointOutcome {
            relay: Some(RelayAction::required(TxVerificationResult::GenerationReset)),
            ..Default::default()
        },
    }
}

fn compile_acceptance(acceptance: CommittedAcceptance) -> CompiledEndpointOutcome {
    match acceptance {
        CommittedAcceptance::Admission {
            entry,
            status,
            ingress_peer,
        } => {
            let tx_hash = compact_packed(&entry.tx.hash());
            CompiledEndpointOutcome {
                callback: Some(acceptance_callback(&entry, status)),
                relay: Some(RelayAction::ordinary(TxVerificationResult::Ok {
                    original_peer: ingress_peer,
                    tx_hash,
                })),
                ..Default::default()
            }
        }
        CommittedAcceptance::Duplicate {
            tx_hash,
            requesting_peer,
        } => CompiledEndpointOutcome {
            relay: Some(RelayAction::ordinary(TxVerificationResult::Ok {
                original_peer: requesting_peer,
                tx_hash: compact_hash(&tx_hash),
            })),
            ..Default::default()
        },
        CommittedAcceptance::ChainStatusChange { entry, status } => CompiledEndpointOutcome {
            callback: Some(acceptance_callback(&entry, status)),
            ..Default::default()
        },
    }
}

fn compile_rejection(rejection: CommittedRejection) -> CompiledEndpointOutcome {
    match rejection {
        CommittedRejection::Validation {
            tx,
            audience,
            reason,
        } => {
            let should_record = reason.should_record();
            let malformed = reason.is_malformed();
            let relay_allowed = reason.relay_allowed();
            compile_preaccepted_rejection(
                tx,
                audience,
                reason.reject().clone(),
                should_record,
                malformed,
                relay_allowed,
            )
        }
        CommittedRejection::Membership {
            tx,
            audience,
            reason,
        } => {
            let public = reason.into_public();
            compile_preaccepted_rejection_from_public(tx, audience, public)
        }
        CommittedRejection::Replaced {
            entry,
            audience: _,
            winner,
        } => compile_accepted_rejection(
            entry,
            Reject::RBFRejected(format!("replaced by tx {}", winner.0)),
        ),
        CommittedRejection::CapacityEvicted {
            entry,
            audience: _,
            fee_rate,
        } => compile_accepted_rejection(
            entry,
            Reject::Full(format!("the fee_rate for this transaction is: {fee_rate}")),
        ),
        CommittedRejection::ChainConflict {
            owner,
            audience,
            out_point,
        } => {
            let public = Reject::Resolve(OutPointError::Dead(out_point));
            match owner {
                CommittedConflictOwner::PreAccepted(tx) => {
                    compile_preaccepted_rejection_from_public(tx, audience, public)
                }
                CommittedConflictOwner::Accepted(entry) => {
                    compile_accepted_rejection(entry, public)
                }
            }
        }
        #[cfg(test)]
        CommittedRejection::Foundation {
            tx,
            audience,
            reason,
        } => {
            let public = super::rejection::CommittedPublicReject::from(reason);
            compile_preaccepted_rejection_from_public(tx, audience, public.reject().clone())
        }
    }
}

fn compile_preaccepted_rejection_from_public(
    tx: Arc<ckb_types::core::TransactionView>,
    audience: RejectionAudience,
    reject: Reject,
) -> CompiledEndpointOutcome {
    let should_record = reject.should_recorded();
    let malformed = reject.is_malformed_tx();
    let relay_allowed = reject.is_allowed_relay();
    compile_preaccepted_rejection(
        tx,
        audience,
        reject,
        should_record,
        malformed,
        relay_allowed,
    )
}

fn compile_preaccepted_rejection(
    tx: Arc<ckb_types::core::TransactionView>,
    audience: RejectionAudience,
    reject: Reject,
    should_record: bool,
    malformed: bool,
    relay_allowed: bool,
) -> CompiledEndpointOutcome {
    let tx_hash = compact_packed(&tx.hash());
    let recent_reject = should_record.then(|| RecentRejectAction {
        tx_hash: tx_hash.clone(),
        reject: reject.clone(),
    });
    let ban = if malformed {
        audience.blame_peer.map(|peer| BanAction {
            peer,
            duration: Duration::from_secs(MALFORMED_TX_BAN_SECONDS),
            reason: bounded_commit_ban_reason(&reject),
        })
    } else {
        None
    };
    // A duplicate is represented by `CommittedAcceptance::Duplicate`. Keep
    // the existing relayer contract defensive here as well: if a future
    // validation producer misclassifies one, it must not turn an accepted
    // transaction into a negative peer-filter result.
    let relay = (audience.ingress_peer.is_some()
        && relay_allowed
        && !matches!(&reject, Reject::Duplicated(_)))
    .then(|| {
        RelayAction::ordinary(TxVerificationResult::Reject {
            tx_hash: tx_hash.clone(),
        })
    });
    CompiledEndpointOutcome {
        recent_reject,
        callback: None,
        ban,
        relay,
    }
}

fn compile_accepted_rejection(
    entry: CommittedEntrySnapshot,
    reject: Reject,
) -> CompiledEndpointOutcome {
    let tx_hash = compact_packed(&entry.tx.hash());
    let recent_reject = reject.should_recorded().then(|| RecentRejectAction {
        tx_hash: tx_hash.clone(),
        reject: reject.clone(),
    });
    let relay =
        (reject.is_allowed_relay() && !matches!(&reject, Reject::Duplicated(_))).then(|| {
            RelayAction::ordinary(TxVerificationResult::Reject {
                tx_hash: tx_hash.clone(),
            })
        });
    CompiledEndpointOutcome {
        recent_reject,
        callback: Some(CallbackEvent::Reject(callback_snapshot(&entry), reject)),
        ban: None,
        relay,
    }
}

fn acceptance_callback(entry: &CommittedEntrySnapshot, status: AcceptedStatus) -> CallbackEvent {
    let snapshot = callback_snapshot(entry);
    match status {
        AcceptedStatus::Pending | AcceptedStatus::Gap => CallbackEvent::Pending(snapshot),
        AcceptedStatus::Proposed => CallbackEvent::Proposed(snapshot),
    }
}

fn callback_snapshot(entry: &CommittedEntrySnapshot) -> TxEntrySnapshot {
    TxEntrySnapshot {
        transaction: entry.tx.as_ref().clone(),
        cycles: entry.cycles,
        size: entry.size,
        fee: entry.fee,
        ancestors_size: entry.ancestors_size,
        ancestors_fee: entry.ancestors_fee,
        ancestors_cycles: entry.ancestors_cycles,
        ancestors_count: entry.ancestors_count,
        descendants_fee: entry.descendants_fee,
        descendants_size: entry.descendants_size,
        descendants_cycles: entry.descendants_cycles,
        descendants_count: entry.descendants_count,
        timestamp: entry.timestamp,
    }
}

fn compact_hash(hash: &RawTxHash) -> Byte32 {
    compact_packed(&hash.0)
}

#[derive(Debug)]
enum BlockingEffectFailure {
    TimedOut,
    Task(tokio::task::JoinError),
}

impl std::fmt::Display for BlockingEffectFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut => formatter.write_str("timed out"),
            Self::Task(join_error) => write!(formatter, "task failed: {join_error}"),
        }
    }
}

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
        Ok(Err(join_error)) => Err(BlockingEffectFailure::Task(join_error)),
        Err(_) => Err(BlockingEffectFailure::TimedOut),
    }
}

struct RetainedEffectLease {
    runtime: AuthorityRuntime,
    lease: Option<EffectLease>,
}

impl RetainedEffectLease {
    fn new(runtime: AuthorityRuntime, lease: EffectLease) -> Self {
        Self {
            runtime,
            lease: Some(lease),
        }
    }

    fn current(&self) -> Option<(usize, EffectEndpoint, CommittedEffect)> {
        self.lease
            .as_ref()
            .and_then(EffectLease::current)
            .map(|work| (work.effect_index, work.endpoint, work.effect.clone()))
    }

    fn mark_current_processed(&mut self) -> Result<bool, AuthorityEffectPublisherFault> {
        self.lease
            .as_mut()
            .ok_or_else(|| {
                AuthorityEffectPublisherFault::authority(PlanError::Fault(
                    super::plan::AuthorityFault::EffectProjection,
                ))
            })?
            .mark_current_processed()
            .map_err(AuthorityEffectPublisherFault::progress)
    }

    fn settle(
        mut self,
        disposition: EndpointDisposition,
    ) -> Result<(), AuthorityEffectPublisherFault> {
        let Some(lease) = self.lease.take() else {
            return Err(AuthorityEffectPublisherFault::authority(PlanError::Fault(
                super::plan::AuthorityFault::EffectProjection,
            )));
        };
        let completed = match lease.into_complete() {
            Ok(completed) => completed,
            Err(failure) => {
                let (error, lease) = failure.into_parts();
                self.lease = Some(lease);
                return Err(AuthorityEffectPublisherFault::progress(error));
            }
        };
        let settlement = match disposition {
            EndpointDisposition::Published => completed.published(),
            EndpointDisposition::CircuitDisposed => completed.circuit_disposed(),
        };
        self.runtime
            .settle_effect(settlement)
            .map_err(AuthorityEffectPublisherFault::settlement)
    }
}

impl Drop for RetainedEffectLease {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        if let Err(failure) = self.runtime.settle_effect(lease.retain()) {
            error!(
                "failed to retain cancelled tx-pool effect lease: {:?}",
                failure.error()
            );
        }
    }
}

/// Drain the sole authority effect sequence into stable external endpoints.
///
/// There is intentionally no cancellation token: normal shutdown first stops
/// every producer, closes the authority effect log, and lets this loop drain
/// to `None`. Task abortion still cannot orphan the active head because the
/// move-only guard synchronously returns the complete lease from `Drop`.
pub(crate) async fn run_authority_effect_publisher(
    runtime: AuthorityRuntime,
    endpoints: AuthorityEffectEndpoints,
) -> Result<(), AuthorityEffectPublisherFault> {
    let Some(_claim) = runtime.claim_effect_publisher() else {
        return Err(AuthorityEffectPublisherFault::concurrent_consumer());
    };
    loop {
        let Some(lease) = runtime
            .wait_effect_checkout()
            .await
            .map_err(AuthorityEffectPublisherFault::authority)?
        else {
            info!("tx-pool authority effect publisher drained and exited");
            return Ok(());
        };
        let mut retained = RetainedEffectLease::new(runtime.clone(), lease);
        let mut disposition = EndpointDisposition::Published;
        let mut relay_reconciled = false;
        'batch: while let Some((effect_index, mut endpoint, effect)) = retained.current() {
            let mut outcome = compile_committed_effect(effect);
            loop {
                disposition = disposition.join(
                    endpoints
                        .publish_endpoint(&mut outcome, endpoint, &mut relay_reconciled)
                        .await?,
                );
                if retained.mark_current_processed()? {
                    break 'batch;
                }
                let Some((next_effect_index, next_endpoint, _)) = retained.current() else {
                    return Err(AuthorityEffectPublisherFault::progress(
                        EffectProgressError::Incomplete,
                    ));
                };
                if next_effect_index != effect_index {
                    break;
                }
                endpoint = next_endpoint;
            }
        }
        retained.settle(disposition)?;
    }
}
