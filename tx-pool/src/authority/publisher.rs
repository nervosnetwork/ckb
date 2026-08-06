//! Exhaustive post-commit publication for the unified authority.
//!
//! The compiler in this module consumes only [`CommittedEffect`] values. It
//! never rereads transaction authority after Apply, so a later admission,
//! reorg, or replacement cannot change the meaning of an older outcome. The
//! publisher owns one claim-bound read receipt at a time; cancellation settles
//! its tentative cursor before the task can release the sole publisher claim.

use super::rejection::{bounded_commit_ban_reason, serialized_recent_reject};
use super::{
    effect::{
        CommittedAcceptance, CommittedConflictOwner, CommittedEffect, CommittedEntrySnapshot,
        CommittedRejection, EffectEndpoint, RejectionAudience,
    },
    relay::{
        AuthorityRelaySink, RelayMailboxDisposition, RelayParentProjectionError,
        project_parent_request,
    },
    runtime::{
        AuthorityEffectPublicationFault, AuthorityEffectPublicationLease,
        AuthorityEffectPublisherClaim, AuthorityRuntime,
    },
    state::{AcceptedStatus, RawTxHash},
};
use crate::{
    callback::{CallbackEvent, Callbacks},
    component::{entry::TxEntrySnapshot, recent_reject::RecentReject},
    error::Reject,
    network::TxPoolNetworkHandle,
    service::TxVerificationResult,
    util::compact_packed,
};
use ckb_logger::{error, info};
use ckb_types::packed::Byte32;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const EXTERNAL_EFFECT_TIMEOUT: Duration = Duration::from_secs(1);

pub(in crate::authority) type AuthorityEffectPublisherFault = AuthorityEffectPublicationFault;

/// Stable foreign endpoints and their one-way circuit breakers.
///
/// Circuits are operational projections, not transaction state. Opening one
/// may discard only that endpoint's observational detail; it cannot reverse a
/// committed transition. Every potentially blocking endpoint uses Tokio's
/// existing bounded blocking execution boundary. Sequential publication plus
/// a one-way circuit bounds each endpoint to at most one detached operation;
/// no extra service task or shutdown capability is required. The move-only
/// publisher owns these plain circuit bits; they are not shared state.
pub(crate) struct AuthorityEffectEndpoints {
    network: TxPoolNetworkHandle,
    relay: AuthorityRelaySink,
    callbacks: Arc<Callbacks>,
    recent_reject: Option<Arc<RecentReject>>,
    callback_circuit_open: bool,
    network_circuit_open: bool,
    recent_reject_circuit_open: bool,
    relay_circuit_open: bool,
}

impl AuthorityEffectEndpoints {
    pub(crate) fn new(
        network: TxPoolNetworkHandle,
        relay: AuthorityRelaySink,
        callbacks: Arc<Callbacks>,
        recent_reject: Option<Arc<RecentReject>>,
    ) -> Self {
        Self {
            network,
            relay,
            callbacks,
            recent_reject,
            callback_circuit_open: false,
            network_circuit_open: false,
            recent_reject_circuit_open: false,
            relay_circuit_open: false,
        }
    }

    async fn publish_endpoint(
        &mut self,
        outcome: &mut CompiledEndpointOutcome,
        endpoint: EffectEndpoint,
    ) -> EndpointDisposition {
        match endpoint {
            EffectEndpoint::RecentReject => match outcome.recent_reject.take() {
                Some(recent) => self.publish_recent_reject(recent).await,
                None => EndpointDisposition::Published,
            },
            EffectEndpoint::Callback => match outcome.callback.take() {
                Some(callback) => self.publish_callback(callback).await,
                None => EndpointDisposition::Published,
            },
            EffectEndpoint::Ban => match outcome.ban.take() {
                Some(ban) => self.publish_ban(ban).await,
                None => EndpointDisposition::Published,
            },
            EffectEndpoint::Relay => {
                let Some(relay) = outcome.relay.take() else {
                    return EndpointDisposition::Published;
                };
                match self.publish_relay(relay) {
                    RelayDisposition::Exact => EndpointDisposition::Published,
                    RelayDisposition::Reconciled => EndpointDisposition::Published,
                    RelayDisposition::CircuitDisposed => EndpointDisposition::CircuitDisposed,
                }
            }
        }
    }

    async fn publish_callback(&mut self, event: CallbackEvent) -> EndpointDisposition {
        let registered = match &event {
            CallbackEvent::Pending(_) => self.callbacks.pending.is_some(),
            CallbackEvent::Proposed(_) => self.callbacks.proposed.is_some(),
            CallbackEvent::Reject(_, _) => self.callbacks.reject.is_some(),
        };
        if !registered {
            return EndpointDisposition::Published;
        }
        if self.callback_circuit_open {
            return EndpointDisposition::CircuitDisposed;
        }
        let callbacks = Arc::clone(&self.callbacks);
        if let Err(failure) = run_blocking_effect(move || {
            crate::callback::with_callback_context(|| callbacks.publish(&event));
        })
        .await
        {
            self.callback_circuit_open = true;
            error!("tx-pool callback endpoint failed; circuit opened: {failure}");
            return EndpointDisposition::CircuitDisposed;
        }
        EndpointDisposition::Published
    }

    async fn publish_ban(&mut self, ban: BanAction) -> EndpointDisposition {
        if self.network_circuit_open {
            return EndpointDisposition::CircuitDisposed;
        }
        let network = Arc::clone(&self.network);
        let lease = ban.lease;
        let published = run_blocking_effect(move || {
            // Compute the lease remainder inside the blocking task, at the
            // actual foreign-call boundary. Queueing delay in Tokio's blocking
            // pool must not silently extend the authority-owned deadline.
            if let Some(duration) = lease.remaining_at(Instant::now()) {
                let peer = lease.peer();
                network.ban_peer(peer, duration, ban.reason.clone());
                report_malformed_peer_ban(peer, duration, &ban.reason);
            }
        })
        .await;
        if let Err(failure) = published {
            self.network_circuit_open = true;
            error!("tx-pool network effect failed; circuit opened: {failure}");
            EndpointDisposition::CircuitDisposed
        } else {
            EndpointDisposition::Published
        }
    }

    async fn publish_recent_reject(&mut self, recent: RecentRejectAction) -> EndpointDisposition {
        let Some(store) = self.recent_reject.as_ref().map(Arc::clone) else {
            return EndpointDisposition::Published;
        };
        if self.recent_reject_circuit_open {
            return EndpointDisposition::CircuitDisposed;
        }
        let serialized = match serialized_recent_reject(&recent.reject) {
            Ok(serialized) => serialized,
            Err(encoding_error) => {
                self.recent_reject_circuit_open = true;
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
                self.recent_reject_circuit_open = true;
                error!("{store_error}");
                EndpointDisposition::CircuitDisposed
            }
            Err(failure) => {
                self.recent_reject_circuit_open = true;
                error!("tx-pool recent-reject effect failed; circuit opened: {failure}");
                EndpointDisposition::CircuitDisposed
            }
        }
    }

    pub(super) fn publish_relay(&mut self, action: RelayAction) -> RelayDisposition {
        if self.relay_circuit_open {
            return RelayDisposition::CircuitDisposed;
        }
        match self.relay.publish(action.result) {
            RelayMailboxDisposition::Exact => RelayDisposition::Exact,
            RelayMailboxDisposition::Reconciled => RelayDisposition::Reconciled,
            RelayMailboxDisposition::Unavailable => {
                error!(
                    "tx-pool relay mailbox could not retain required detail; bounded Remote availability degradation recorded"
                );
                RelayDisposition::CircuitDisposed
            }
            RelayMailboxDisposition::Disconnected => {
                self.relay_circuit_open = true;
                error!(
                    "tx-pool relayer mailbox disconnected; relay circuit opened and committed effects will continue draining"
                );
                RelayDisposition::CircuitDisposed
            }
        }
    }
}

fn report_malformed_peer_ban(peer: ckb_network::PeerIndex, duration: Duration, reason: &str) {
    #[cfg(not(feature = "with_sentry"))]
    let _ = (peer, duration, reason);

    #[cfg(feature = "with_sentry")]
    sentry::with_scope(
        |scope| scope.set_fingerprint(Some(&["ckb-tx-pool", "receive-invalid-remote-tx"])),
        || {
            sentry::capture_message(
                &format!(
                    "Ban peer {peer} for {} seconds, reason: {reason}",
                    duration.as_secs()
                ),
                sentry::Level::Info,
            )
        },
    );
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
    CircuitDisposed,
}

pub(super) struct RelayAction {
    pub(super) result: TxVerificationResult,
}

impl RelayAction {
    pub(super) fn new(result: TxVerificationResult) -> Self {
        Self { result }
    }
}

pub(super) struct RecentRejectAction {
    pub(super) tx_hash: Byte32,
    pub(super) reject: Reject,
}

pub(super) struct BanAction {
    lease: super::ban::PeerBanLease,
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
            relay: Some(RelayAction::new(TxVerificationResult::Ok {
                original_peer: Some(ingress_peer),
                tx_hash: compact_hash(&tx_hash),
            })),
            ..Default::default()
        },
        CommittedEffect::PeerCohortRevoked(revocation) => {
            let recent_reject = revocation.culprit().and_then(|culprit| {
                culprit
                    .reason()
                    .should_record()
                    .then(|| RecentRejectAction {
                        tx_hash: compact_hash(culprit.tx_hash()),
                        reject: culprit.reason().reject().clone(),
                    })
            });
            let ban = revocation.culprit().map(|culprit| BanAction {
                lease: revocation.lease(),
                reason: bounded_commit_ban_reason(culprit.reason().reject()),
            });
            CompiledEndpointOutcome {
                recent_reject,
                ban,
                // Cohort removal has no transaction tombstone. A required
                // reset clears every stale known/pending projection so the
                // same raw transaction can be supplied by another peer.
                relay: Some(RelayAction::new(TxVerificationResult::GenerationReset)),
                ..Default::default()
            }
        }
        CommittedEffect::RemoteExpired { tx_hash } => CompiledEndpointOutcome {
            relay: Some(RelayAction::new(TxVerificationResult::Reject {
                tx_hash: compact_hash(&tx_hash),
            })),
            ..Default::default()
        },
        CommittedEffect::RemoteIngressReleased(release) => CompiledEndpointOutcome {
            relay: Some(RelayAction::new(TxVerificationResult::Reject {
                tx_hash: compact_hash(release.tx_hash()),
            })),
            ..Default::default()
        },
        CommittedEffect::ParentTransactionsRequested(request) => {
            let relay = match project_parent_request(&request) {
                Ok(result) => result,
                Err(RelayParentProjectionError::Allocation) => {
                    error!(
                        "tx-pool could not materialize a committed parent request; scheduling authoritative relay reconciliation"
                    );
                    TxVerificationResult::GenerationReset
                }
            };
            CompiledEndpointOutcome {
                // UnknownParents is the only variable-size relay result whose
                // detail is required for dependency recovery. The bounded
                // mailbox or a fallible projection allocation reconciles from
                // the authoritative waiting level instead of losing the only
                // external recovery action.
                relay: Some(RelayAction::new(relay)),
                ..Default::default()
            }
        }
        CommittedEffect::GenerationReset => CompiledEndpointOutcome {
            relay: Some(RelayAction::new(TxVerificationResult::GenerationReset)),
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
                relay: Some(RelayAction::new(TxVerificationResult::Ok {
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
            relay: Some(RelayAction::new(TxVerificationResult::Ok {
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
    let public = rejection.public_reject();
    match rejection {
        CommittedRejection::Validation {
            tx,
            audience,
            reason: _,
        }
        | CommittedRejection::Membership {
            tx,
            audience,
            reason: _,
        } => compile_preaccepted_rejection(tx, audience, public),
        CommittedRejection::Replaced { entry, winner: _ } => {
            compile_accepted_rejection(entry, public)
        }
        CommittedRejection::CapacityEvicted { entry, fee_rate: _ } => {
            compile_accepted_rejection(entry, public)
        }
        CommittedRejection::Expired { entry } => compile_accepted_rejection(entry, public),
        CommittedRejection::ChainConflict {
            owner,
            out_point: _,
        } => match owner {
            CommittedConflictOwner::PreAccepted { tx, audience } => {
                compile_preaccepted_rejection(tx, audience, public)
            }
            CommittedConflictOwner::Accepted(entry) => compile_accepted_rejection(entry, public),
        },
    }
}

fn compile_preaccepted_rejection(
    tx: Arc<ckb_types::core::TransactionView>,
    audience: RejectionAudience,
    public: super::rejection::CommittedPublicReject,
) -> CompiledEndpointOutcome {
    let should_record = public.should_record();
    let relay_allowed = public.relay_allowed();
    let reject = public.reject().clone();
    let tx_hash = compact_packed(&tx.hash());
    let recent_reject = should_record.then(|| RecentRejectAction {
        tx_hash: tx_hash.clone(),
        reject: reject.clone(),
    });
    // A duplicate is represented by `CommittedAcceptance::Duplicate`. Keep
    // the existing relayer contract defensive here as well: if a future
    // validation producer misclassifies one, it must not turn an accepted
    // transaction into a negative peer-filter result.
    let relay =
        (audience.has_ingress() && relay_allowed && !matches!(&reject, Reject::Duplicated(_)))
            .then(|| {
                RelayAction::new(TxVerificationResult::Reject {
                    tx_hash: tx_hash.clone(),
                })
            });
    CompiledEndpointOutcome {
        recent_reject,
        callback: None,
        ban: None,
        relay,
    }
}

fn compile_accepted_rejection(
    entry: CommittedEntrySnapshot,
    public: super::rejection::CommittedPublicReject,
) -> CompiledEndpointOutcome {
    let reject = public.reject().clone();
    let tx_hash = compact_packed(&entry.tx.hash());
    let recent_reject = public.should_record().then(|| RecentRejectAction {
        tx_hash: tx_hash.clone(),
        reject: reject.clone(),
    });
    let relay = (public.relay_allowed() && !matches!(&reject, Reject::Duplicated(_))).then(|| {
        RelayAction::new(TxVerificationResult::Reject {
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

/// Publish and settle one immutable committed effect batch.
///
/// The span is intentionally scoped to one receipt rather than the permanent
/// publisher task. It observes post-commit I/O only and never overlaps an
/// authority guard.
#[cfg_attr(
    feature = "profiling",
    tracing::instrument(
        name = "tx_pool.effects.publish",
        target = "ckb_tx_pool_profile",
        level = "trace",
        skip_all
    )
)]
async fn publish_committed_effect_batch(
    endpoints: &mut AuthorityEffectEndpoints,
    mut publication: AuthorityEffectPublicationLease<'_, '_>,
) -> Result<(), AuthorityEffectPublisherFault> {
    let mut disposition = EndpointDisposition::Published;
    'batch: while let Some(work) = publication.current() {
        let effect_index = work.effect_index;
        let mut endpoint = work.endpoint;
        let mut outcome = compile_committed_effect(work.effect.clone());
        loop {
            disposition =
                disposition.join(endpoints.publish_endpoint(&mut outcome, endpoint).await);
            if publication
                .mark_current_processed()
                .map_err(AuthorityEffectPublicationFault::Progress)?
            {
                break 'batch;
            }
            let Some(next) = publication.current() else {
                return Err(AuthorityEffectPublicationFault::Progress(
                    super::effect::EffectProgressError::Incomplete,
                ));
            };
            if next.effect_index != effect_index {
                break;
            }
            endpoint = next.endpoint;
        }
    }
    match disposition {
        EndpointDisposition::Published => publication.publish(),
        EndpointDisposition::CircuitDisposed => publication.circuit_dispose(),
    }
}

/// Drain with the move-only claim acquired before any topology task starts.
/// This entry point makes duplicate-consumer failure a construction outcome,
/// rather than an asynchronous task exit after partial startup.
pub(in crate::authority) async fn run_claimed_authority_effect_publisher(
    runtime: AuthorityRuntime,
    mut endpoints: AuthorityEffectEndpoints,
    mut claim: AuthorityEffectPublisherClaim,
) -> Result<(), AuthorityEffectPublisherFault> {
    loop {
        let Some(lease) = runtime.wait_effect_publication(&mut claim).await else {
            info!("tx-pool authority effect publisher drained and exited");
            return Ok(());
        };
        publish_committed_effect_batch(&mut endpoints, lease).await?;
    }
}

#[cfg(test)]
#[path = "tests/support/publisher.rs"]
pub(in crate::authority) mod test_support;
