//! Narrow service boundary for the unified authority kernel.
//!
//! This module is the only place where production-shaped service operations
//! may call the authority's private capabilities. It owns no transaction
//! facts: all mutation remains in [`AuthorityRuntime`], while this adapter
//! performs exhaustive conversion to closed service outcomes after Plan/Apply.

pub(crate) use super::relay::AuthorityRelaySink;
use super::{
    chain_boundary::{
        CandidateUncleCollection, ChainBoundaryError, ChainGenerationReplacement,
        ChainUpdateRequest, CommittedChainUpdate,
    },
    effect::ParentTransactionRequest,
    ingress::{
        BoundedTransaction, RetainedAdmissionBatch, RetainedIngressAttempt,
        RetainedIngressBackpressure, RetainedIngressBoundaryError, proposal, remote,
    },
    plan::AuthorityFault,
    publisher::AuthorityEffectEndpoints,
    query::{
        AcceptedTransactionsWithCycles, AuthorityPoolSummary, AuthorityQueryError,
        AuthorityTransactionLookup, AuthorityTransactionStatusLookup, CompactBlockReadReceipt,
        FeeEstimateReadReceipt, LiveCellReadReceipt, PersistenceReceipt,
    },
    read::{RelayParentRebuildCursor, RelayParentRebuildError},
    relay::{
        AuthorityRelayReceiver, RelayMailboxConfigError, production_authority_relay_mailbox,
        project_parent_request,
    },
    resolver::{DirectComputationError, VerificationCacheUpdate},
    resources::ResourceCapacityWaitIdentity,
    runtime::{
        AuthorityAdministrationError, AuthorityDirectAdmissionError,
        AuthorityDirectAdmissionExecution, AuthorityDirectRejectionExecution,
        AuthorityDirectResolutionOutcome, AuthorityDirectVerificationOutcome,
        AuthorityGenerationReplacementError, AuthorityLocalAdmissionOutcome,
        AuthorityRecentRejectReadError, AuthorityRelayParentReader, AuthorityRuntime,
        AuthorityTestAcceptOutcome, DirectAdmissionRejectionKind,
        RetainedIngressBatchFailureReason, RuntimeConfigError,
    },
    template_driver::{
        AuthorityBlockAssembler, AuthorityTemplateDriverFault, AuthorityTemplateReadFailure,
    },
    topology::{
        AuthorityDerivedTaskFailure, AuthorityGenerationFault, AuthorityShutdownStatus,
        AuthorityTaskTopology, AuthorityTopologyEvent, AuthorityTopologyStartError,
    },
    worker::AuthorityWorkerFaultKind,
};
#[cfg(any(test, feature = "internal"))]
use super::{
    packing::TemplatePackingLimits,
    runtime::{AuthorityInternalPlugError, AuthorityInternalPlugOutcome},
    state::AcceptedStatus,
    template::TemplateReadError,
};
#[cfg(any(test, feature = "internal"))]
use crate::{PlugTarget, component::entry::TxEntry};
use crate::{
    block_assembler::BlockAssembler,
    callback::Callbacks,
    dependency_sort::DependencySortError,
    error::Reject,
    network::TxPoolNetworkHandle,
    service::{
        ChainControl, ChainReorgArgs, DEFAULT_CHANNEL_SIZE, OneshotSender, Request,
        TxVerificationResult, respond,
    },
};
use ckb_app_config::TxPoolConfig;
use ckb_async_runtime::Handle;
use ckb_error::AnyError;
use ckb_fee_estimator::FeeEstimator;
use ckb_network::PeerIndex;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::CancellationToken;
use ckb_types::{
    core::{EstimateMode, FeeRate, TransactionView, tx_pool::EntryCompleted},
    packed::{Byte32, OutPoint, ProposalShortId},
};
use ckb_util::Mutex;
use ckb_verification::cache::TxVerificationCache;
use std::{
    collections::{HashMap, VecDeque},
    num::NonZeroUsize,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{RwLock, mpsc, watch};

/// Startup failures before any authority task is spawned or service ingress is
/// opened. A caller may abandon the whole construction without quiescing a
/// partial generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityServiceStartError {
    RuntimeConfiguration,
    RelayConfiguration,
    TemplateConstruction,
    Cancelled,
    EffectPublisherClaimed,
    WorkerAllocation,
}

/// Structural or operational failure after the caller supplied a valid
/// service command. Legal policy rejection and bounded ingress pressure use
/// their own outcome types instead of this fault domain.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AuthorityServiceError {
    Cancelled,
    BlockAssemblerDisabled,
    TemplateUnavailable,
    ResourceUnavailable,
    EffectCapacity,
    LifecycleClosed,
    CompetingProgress,
    Integrity(AuthorityGenerationInvalidity),
}

/// Exact Remote responder prefix completed by authority. A later operational
/// failure cannot erase this observation: dispatch acknowledges only this
/// prefix and drops the uncommitted suffix so the relayer can release its
/// matching known-filter handoffs.
pub(crate) struct RemoteIngressBatchProgress {
    completed: usize,
    error: Option<AuthorityServiceError>,
}

impl RemoteIngressBatchProgress {
    fn complete(completed: usize) -> Self {
        Self {
            completed,
            error: None,
        }
    }

    fn failed(completed: usize, error: AuthorityServiceError) -> Self {
        Self {
            completed,
            error: Some(error),
        }
    }

    pub(crate) fn into_parts(self) -> (usize, Option<AuthorityServiceError>) {
        (self.completed, self.error)
    }

    pub(crate) fn into_checked_parts(
        self,
        expected: usize,
    ) -> (usize, Option<AuthorityServiceError>) {
        let (completed, error) = self.into_parts();
        if completed > expected || (error.is_none() && completed != expected) {
            (
                completed,
                Some(AuthorityServiceError::from(
                    AuthorityFault::MembershipProjection,
                )),
            )
        } else {
            (completed, error)
        }
    }
}

/// Closed programmer-defect domain that alone can invalidate one authority
/// generation. Keeping it out of [`AuthorityServiceError`]'s operational
/// variants makes classification exhaustive when either enum evolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityIntegrityFault {
    InvalidChainEvidence,
    EffectLifecycleClosed,
    Authority(AuthorityFault),
}

/// Exhaustive failure domain for the sole ordered chain-update consumer.
///
/// Allocation pressure consumes the exact returned request or command in one
/// ordered empty-generation replacement at the requested snapshot. It is
/// never retried against an unchanged allocator premise. Consequently, once a
/// chain update returns, it can only have observed generation cancellation or
/// a structural contradiction.
/// Keeping this narrower than `AuthorityServiceError` prevents a future
/// operational service variant from silently terminating the ordered driver.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AuthorityChainUpdateError {
    Cancelled,
    Integrity(AuthorityGenerationInvalidity),
}

impl AuthorityChainUpdateError {
    fn integrity(fault: AuthorityIntegrityFault) -> Self {
        Self::Integrity(AuthorityGenerationInvalidity::from_integrity(fault))
    }
}

impl From<AuthorityFault> for AuthorityIntegrityFault {
    fn from(fault: AuthorityFault) -> Self {
        Self::Authority(fault)
    }
}

impl From<AuthorityIntegrityFault> for AuthorityServiceError {
    fn from(fault: AuthorityIntegrityFault) -> Self {
        Self::Integrity(AuthorityGenerationInvalidity::from_integrity(fault))
    }
}

impl From<AuthorityFault> for AuthorityServiceError {
    fn from(fault: AuthorityFault) -> Self {
        Self::from(AuthorityIntegrityFault::from(fault))
    }
}

/// Move-only proof that a service error is a structural contradiction and
/// therefore makes this authority generation ineligible for persistence.
/// Operational outcomes cannot construct this type.
#[must_use = "generation invalidity must be retained until controlled shutdown"]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AuthorityGenerationInvalidity(AuthorityIntegrityFault);

impl AuthorityGenerationInvalidity {
    fn from_integrity(fault: AuthorityIntegrityFault) -> Self {
        Self(fault)
    }
}

/// Persistence failures are operational outcomes after a coherent read cut;
/// none can invalidate or roll back authority state.
#[derive(Debug)]
pub(crate) enum AuthorityPersistenceError {
    Snapshot(AuthorityServiceError),
    Sort(
        #[expect(
            dead_code,
            reason = "the exact sort cause is retained for the caller's Debug diagnostics"
        )]
        DependencySortError,
    ),
    Replay(AuthorityServiceError),
    Counter,
    Write(
        #[expect(
            dead_code,
            reason = "the exact storage cause is retained for the caller's Debug diagnostics"
        )]
        AnyError,
    ),
    Join(
        #[expect(
            dead_code,
            reason = "the exact task cause is retained for the caller's Debug diagnostics"
        )]
        tokio::task::JoinError,
    ),
}

#[derive(Debug)]
pub(crate) enum AuthorityDerivedError {
    Authority(AuthorityServiceError),
    External(AnyError),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AuthorityPersistenceReplay {
    pub(crate) loaded: usize,
    pub(crate) stale: usize,
}

/// Immutable construction inputs. This value exists only until the runtime,
/// derived endpoints and task topology have all been constructed.
#[derive(Clone)]
pub(crate) struct AuthorityVerificationCommand {
    sender: Arc<watch::Sender<ChunkCommand>>,
}

impl AuthorityVerificationCommand {
    fn update(&self, command: ChunkCommand) -> Result<(), watch::error::SendError<ChunkCommand>> {
        // Match `watch::Sender::send` at the public controller boundary: a
        // command with no generation receiver is rejected. The conditional
        // mutation itself is serialized by the watch value lock, making Stop
        // absorbing even when an operator Resume races generation shutdown.
        if self.sender.receiver_count() == 0 {
            return Err(watch::error::SendError(command));
        }
        self.sender.send_if_modified(|current| {
            if matches!(current, ChunkCommand::Stop) || *current == command {
                false
            } else {
                *current = command;
                true
            }
        });
        Ok(())
    }

    pub(crate) fn suspend(&self) -> Result<(), watch::error::SendError<ChunkCommand>> {
        self.update(ChunkCommand::Suspend)
    }

    pub(crate) fn resume(&self) -> Result<(), watch::error::SendError<ChunkCommand>> {
        self.update(ChunkCommand::Resume)
    }

    pub(in crate::authority) fn stop(&self) {
        self.sender.send_if_modified(|current| {
            if matches!(current, ChunkCommand::Stop) {
                false
            } else {
                *current = ChunkCommand::Stop;
                true
            }
        });
    }
}

#[cfg(test)]
#[path = "tests/support/service.rs"]
mod test_support;

pub(crate) struct AuthorityVerificationControl {
    command: AuthorityVerificationCommand,
    receiver: watch::Receiver<ChunkCommand>,
}

impl AuthorityVerificationControl {
    pub(crate) fn channel(initial: ChunkCommand) -> (Self, AuthorityVerificationCommand) {
        let (sender, receiver) = watch::channel(initial);
        let command = AuthorityVerificationCommand {
            sender: Arc::new(sender),
        };
        (
            Self {
                command: command.clone(),
                receiver,
            },
            command,
        )
    }

    fn receiver(&self) -> watch::Receiver<ChunkCommand> {
        self.receiver.clone()
    }

    pub(in crate::authority) fn into_parts(
        self,
    ) -> (AuthorityVerificationCommand, watch::Receiver<ChunkCommand>) {
        (self.command, self.receiver)
    }
}

pub(crate) struct AuthorityServiceInputs {
    pub(crate) bootstrap: AuthorityServiceBootstrap,
    pub(crate) block_assembler: Option<BlockAssembler>,
    pub(crate) verification_cache: Arc<RwLock<TxVerificationCache>>,
    pub(crate) callbacks: Callbacks,
    pub(crate) network: TxPoolNetworkHandle,
    pub(crate) persistence_writer: Arc<crate::persisted::PersistenceWriter>,
    pub(crate) recent_reject: Option<Arc<crate::component::recent_reject::RecentReject>>,
    pub(crate) fee_estimator: FeeEstimator,
    pub(crate) chain_control_receiver: mpsc::Receiver<ChainControl>,
    pub(crate) verification_control: AuthorityVerificationControl,
    pub(crate) cancel: CancellationToken,
}

/// One pre-start authority generation paired with its sole relay publisher.
/// Construction consumes the exact configuration and snapshot, so service
/// assembly cannot accidentally bind a drain to a different authority store.
pub(crate) struct AuthorityServiceBootstrap {
    config: TxPoolConfig,
    runtime: AuthorityRuntime,
    relay_sink: AuthorityRelaySink,
}

impl AuthorityServiceBootstrap {
    pub(crate) fn config(&self) -> &TxPoolConfig {
        &self.config
    }
}

/// Cloneable dispatcher-side adapter. The cancellation token is a generation
/// capability for owner-free Local/TestAccept work, not another lifecycle
/// state machine.
#[derive(Clone)]
pub(crate) struct AuthorityService {
    runtime: AuthorityRuntime,
    config: Arc<TxPoolConfig>,
    block_assembler: Option<AuthorityBlockAssembler>,
    verification_cache: Arc<RwLock<TxVerificationCache>>,
    chunk_rx: watch::Receiver<ChunkCommand>,
    cancel: CancellationToken,
    persistence_writer: Arc<crate::persisted::PersistenceWriter>,
    persistence_base: PathBuf,
    recent_reject: Option<Arc<crate::component::recent_reject::RecentReject>>,
    fee_estimator: FeeEstimator,
}

/// Complete, not-yet-exposed service generation returned by atomic assembly.
pub(crate) struct AuthorityServiceAssembly {
    pub(crate) service: AuthorityService,
    pub(crate) generation: AuthorityGeneration,
}

const RELAY_REBUILD_EMPTY_PAGE_BUDGET: usize = 4;
/// Bound one authority read section during rare relay reconciliation while
/// still allowing one sync drain attempt to skip several pages with no
/// missing waiters.
const RELAY_REBUILD_SCAN_ITEMS: usize = 64;

struct AuthorityRelayRebuildState {
    active: bool,
    cursor: Option<RelayParentRebuildCursor>,
    pending: Vec<ParentTransactionRequest>,
    reported_failure: Option<RelayParentRebuildError>,
}

impl AuthorityRelayRebuildState {
    fn new() -> Self {
        Self {
            active: false,
            cursor: None,
            pending: Vec::new(),
            reported_failure: None,
        }
    }

    fn restart(&mut self) {
        self.active = true;
        self.cursor = None;
        self.pending.clear();
        self.reported_failure = None;
    }

    fn report_once(&mut self, failure: RelayParentRebuildError) {
        if self.reported_failure != Some(failure) {
            ckb_logger::error!(
                "tx-pool relay parent reconciliation is temporarily degraded: {failure:?}"
            );
            self.reported_failure = Some(failure);
        }
    }
}

/// Sole receiver of the bounded derived relay projection. A reset starts a
/// bounded, read-only rebuild of every still-live Remote missing-parent level;
/// no forwarding task, retry authority, or unbounded queue is introduced.
pub(crate) struct AuthorityRelayDrain {
    receiver: AuthorityRelayReceiver,
    parents: AuthorityRelayParentReader,
    scan_limit: NonZeroUsize,
    rebuild: Mutex<AuthorityRelayRebuildState>,
}

impl AuthorityRelayDrain {
    pub(in crate::authority) fn new(
        receiver: AuthorityRelayReceiver,
        parents: AuthorityRelayParentReader,
        scan_limit: NonZeroUsize,
    ) -> Self {
        Self {
            receiver,
            parents,
            scan_limit,
            rebuild: Mutex::new(AuthorityRelayRebuildState::new()),
        }
    }

    pub(crate) fn try_recv(&self) -> Option<TxVerificationResult> {
        if let Some(result) = self.receiver.try_recv() {
            if matches!(result, TxVerificationResult::GenerationReset) {
                self.rebuild.lock().restart();
            }
            return Some(result);
        }

        let mut rebuild = self.rebuild.lock();
        for _ in 0..RELAY_REBUILD_EMPTY_PAGE_BUDGET {
            if let Some(request) = rebuild.pending.pop() {
                let result = project_parent_request(&request);
                rebuild.reported_failure = None;
                return Some(result);
            }
            if !rebuild.active {
                return None;
            }

            let cursor = rebuild.cursor.clone();
            match self.parents.page(cursor.clone(), self.scan_limit) {
                Ok(page) => {
                    let (cut, mut requests, next) = page.into_parts();
                    rebuild.reported_failure = None;
                    requests.reverse();
                    rebuild.pending = requests;
                    if next.is_none() && !self.parents.cut_is_current(&cut) {
                        rebuild.restart();
                        continue;
                    }
                    rebuild.active = next.is_some();
                    rebuild.cursor = next;
                }
                Err(RelayParentRebuildError::StaleCut) => rebuild.restart(),
                Err(error) => {
                    rebuild.cursor = cursor;
                    rebuild.report_once(error);
                    return None;
                }
            }
        }
        None
    }

    pub(crate) async fn wait_for_drain(&self) {
        self.receiver.wait_for_drain().await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityGenerationEvent {
    ShutdownRequested,
    DerivedDegraded,
    GenerationInvalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityShutdownOutcome {
    PersistenceEligible,
    PersistenceForbidden,
}

/// Opaque task owner. A generation-invalid event is retained here until the
/// caller consumes this owner through `shutdown`; no service module can lose
/// or reconstruct the linear fault capability.
pub(crate) struct AuthorityGeneration {
    topology: Option<AuthorityTaskTopology>,
    chain_control: Option<tokio::task::JoinHandle<Result<(), AuthorityGenerationInvalidity>>>,
    cancel: CancellationToken,
    invalid: Option<AuthorityServiceGenerationFault>,
}

#[derive(Debug)]
enum AuthorityServiceGenerationFault {
    Authority(AuthorityGenerationFault),
    Service(
        #[expect(
            dead_code,
            reason = "the generation-invalid capability is retained until shutdown and rendered in the operational fault log"
        )]
        AuthorityGenerationInvalidity,
    ),
    ChainControl(
        #[expect(
            dead_code,
            reason = "the ordered-control invalid capability is retained until shutdown and rendered in the operational fault log"
        )]
        AuthorityGenerationInvalidity,
    ),
    ChainControlJoin(tokio::task::JoinError),
    ChainControlTimeout,
}

impl AuthorityGeneration {
    pub(crate) fn invalidate(&mut self, fault: AuthorityGenerationInvalidity) {
        self.retain_invalid(AuthorityServiceGenerationFault::Service(fault));
    }

    pub(crate) async fn next_event(&mut self) -> AuthorityGenerationEvent {
        if self.invalid.is_some() {
            return AuthorityGenerationEvent::GenerationInvalid;
        }
        let event = match (self.topology.as_mut(), self.chain_control.as_mut()) {
            (Some(topology), Some(chain_control)) => {
                tokio::select! {
                    event = topology.next_event() => GenerationBoundaryEvent::Topology(event),
                    result = chain_control => GenerationBoundaryEvent::ChainControl(result),
                }
            }
            (Some(topology), None) => {
                GenerationBoundaryEvent::Topology(topology.next_event().await)
            }
            (None, Some(chain_control)) => {
                GenerationBoundaryEvent::ChainControl(chain_control.await)
            }
            (None, None) => return AuthorityGenerationEvent::ShutdownRequested,
        };
        match event {
            GenerationBoundaryEvent::Topology(event) => self.classify_topology_event(event),
            GenerationBoundaryEvent::ChainControl(result) => {
                self.chain_control = None;
                match result {
                    Ok(Ok(())) => {
                        if !self.cancel.is_cancelled() {
                            crate::metrics::record_failure(
                                crate::metrics::FailureBoundary::WorkerExit,
                            );
                        }
                        AuthorityGenerationEvent::ShutdownRequested
                    }
                    Ok(Err(fault)) => {
                        self.retain_invalid(AuthorityServiceGenerationFault::ChainControl(fault));
                        AuthorityGenerationEvent::GenerationInvalid
                    }
                    Err(error) => {
                        self.retain_invalid(AuthorityServiceGenerationFault::ChainControlJoin(
                            error,
                        ));
                        AuthorityGenerationEvent::GenerationInvalid
                    }
                }
            }
        }
    }

    fn classify_topology_event(
        &mut self,
        event: AuthorityTopologyEvent,
    ) -> AuthorityGenerationEvent {
        match event {
            AuthorityTopologyEvent::ShutdownRequested(_) => {
                if !self.cancel.is_cancelled() {
                    crate::metrics::record_failure(crate::metrics::FailureBoundary::WorkerExit);
                }
                AuthorityGenerationEvent::ShutdownRequested
            }
            AuthorityTopologyEvent::DerivedDegraded(failure) => {
                crate::metrics::record_failure(derived_failure_boundary(&failure));
                ckb_logger::error!(
                    "tx-pool derived authority task degraded while retaining authoritative state: {failure:?}"
                );
                AuthorityGenerationEvent::DerivedDegraded
            }
            AuthorityTopologyEvent::GenerationInvalid(fault) => {
                self.retain_invalid(AuthorityServiceGenerationFault::Authority(fault));
                AuthorityGenerationEvent::GenerationInvalid
            }
        }
    }

    fn retain_invalid(&mut self, fault: AuthorityServiceGenerationFault) {
        if self.invalid.is_none() {
            crate::metrics::record_failure(service_failure_boundary(&fault));
            self.invalid = Some(fault);
        }
    }

    /// Begin the terminal generation phase without consuming the topology.
    /// Handler-owned Direct capabilities may still settle while workers stop;
    /// effects remain open until `shutdown` performs the ordered final join.
    pub(crate) fn begin_shutdown(&self) {
        if let Some(topology) = self.topology.as_ref() {
            topology.begin_shutdown();
        }
        self.cancel.cancel();
    }

    pub(crate) async fn shutdown(mut self, timeout: Duration) -> AuthorityShutdownOutcome {
        self.begin_shutdown();
        if let Some(mut chain_control) = self.chain_control.take() {
            match tokio::time::timeout(timeout, &mut chain_control).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(fault))) => {
                    self.retain_invalid(AuthorityServiceGenerationFault::ChainControl(fault));
                }
                Ok(Err(error)) => {
                    self.retain_invalid(AuthorityServiceGenerationFault::ChainControlJoin(error));
                }
                Err(_) => {
                    chain_control.abort();
                    let _ = chain_control.await;
                    self.retain_invalid(AuthorityServiceGenerationFault::ChainControlTimeout);
                }
            }
        }
        let Some(topology) = self.topology.take() else {
            return AuthorityShutdownOutcome::PersistenceForbidden;
        };
        let invalid = self.invalid.take();
        let failure_already_recorded = invalid.is_some();
        let report = match invalid {
            Some(AuthorityServiceGenerationFault::Authority(fault)) => {
                topology.invalidate_generation(fault).await
            }
            Some(fault) => {
                ckb_logger::error!(
                    "tx-pool ordered chain-control generation failed; persistence is forbidden: {fault:?}"
                );
                topology.retire_invalid_generation().await;
                return AuthorityShutdownOutcome::PersistenceForbidden;
            }
            None => topology.shutdown(timeout).await,
        };
        for failure in report.derived_failures() {
            crate::metrics::record_failure(derived_failure_boundary(failure));
            ckb_logger::error!("tx-pool derived task shutdown failure: {failure:?}");
        }
        match report.status() {
            AuthorityShutdownStatus::PersistenceEligible => {
                AuthorityShutdownOutcome::PersistenceEligible
            }
            AuthorityShutdownStatus::PersistenceForbidden(fault) => {
                if !failure_already_recorded {
                    crate::metrics::record_failure(authority_failure_boundary(fault));
                }
                ckb_logger::error!(
                    "tx-pool authority generation is ineligible for persistence: {fault:?}"
                );
                AuthorityShutdownOutcome::PersistenceForbidden
            }
        }
    }
}

fn service_failure_boundary(
    fault: &AuthorityServiceGenerationFault,
) -> crate::metrics::FailureBoundary {
    match fault {
        AuthorityServiceGenerationFault::Authority(fault) => authority_failure_boundary(fault),
        AuthorityServiceGenerationFault::Service(_)
        | AuthorityServiceGenerationFault::ChainControl(_) => {
            crate::metrics::FailureBoundary::TypedFault
        }
        AuthorityServiceGenerationFault::ChainControlJoin(error) if error.is_panic() => {
            crate::metrics::FailureBoundary::HandlerUnwind
        }
        AuthorityServiceGenerationFault::ChainControlJoin(_)
        | AuthorityServiceGenerationFault::ChainControlTimeout => {
            crate::metrics::FailureBoundary::WorkerExit
        }
    }
}

pub(super) fn authority_failure_boundary(
    fault: &AuthorityGenerationFault,
) -> crate::metrics::FailureBoundary {
    match fault {
        AuthorityGenerationFault::Worker {
            fault:
                AuthorityWorkerFaultKind::Authority(_)
                | AuthorityWorkerFaultKind::Settlement(_)
                | AuthorityWorkerFaultKind::Completion(_)
                | AuthorityWorkerFaultKind::Exchange(_),
            ..
        } => crate::metrics::FailureBoundary::TypedFault,
        AuthorityGenerationFault::Worker {
            fault: AuthorityWorkerFaultKind::LifecycleClosed,
            ..
        }
        | AuthorityGenerationFault::WorkerJoin { .. }
        | AuthorityGenerationFault::ShutdownTimeout => crate::metrics::FailureBoundary::WorkerExit,
        AuthorityGenerationFault::Publisher(_)
        | AuthorityGenerationFault::PublisherJoin(_)
        | AuthorityGenerationFault::PublisherClosed
        | AuthorityGenerationFault::EffectClose(_)
        | AuthorityGenerationFault::EffectDrain => crate::metrics::FailureBoundary::EffectPublisher,
    }
}

pub(super) fn derived_failure_boundary(
    failure: &AuthorityDerivedTaskFailure,
) -> crate::metrics::FailureBoundary {
    match failure {
        AuthorityDerivedTaskFailure::TemplateJoin { error, .. }
        | AuthorityDerivedTaskFailure::VerificationCacheJoin(error)
            if error.is_panic() =>
        {
            crate::metrics::FailureBoundary::HandlerUnwind
        }
        AuthorityDerivedTaskFailure::Template { .. }
        | AuthorityDerivedTaskFailure::TemplateJoin { .. }
        | AuthorityDerivedTaskFailure::TemplateClosed(_)
        | AuthorityDerivedTaskFailure::TemplateTimeout(_)
        | AuthorityDerivedTaskFailure::VerificationCacheJoin(_)
        | AuthorityDerivedTaskFailure::VerificationCacheClosed
        | AuthorityDerivedTaskFailure::VerificationCacheTimeout => {
            crate::metrics::FailureBoundary::WorkerExit
        }
    }
}

enum GenerationBoundaryEvent {
    Topology(AuthorityTopologyEvent),
    ChainControl(Result<Result<(), AuthorityGenerationInvalidity>, tokio::task::JoinError>),
}

impl AuthorityService {
    #[cfg(test)]
    pub(in crate::authority) fn runtime_for_foundation(&self) -> AuthorityRuntime {
        self.runtime.clone()
    }

    /// Classify one closed service error at the compatibility boundary.
    /// Only structural contradictions can yield the linear invalidity proof.
    pub(crate) fn settle_operation_error(
        error: AuthorityServiceError,
    ) -> Result<(), AuthorityGenerationInvalidity> {
        match error {
            AuthorityServiceError::Integrity(invalidity) => Err(invalidity),
            operational @ (AuthorityServiceError::Cancelled
            | AuthorityServiceError::BlockAssemblerDisabled
            | AuthorityServiceError::TemplateUnavailable
            | AuthorityServiceError::ResourceUnavailable
            | AuthorityServiceError::EffectCapacity
            | AuthorityServiceError::LifecycleClosed
            | AuthorityServiceError::CompetingProgress) => {
                ckb_logger::debug!(
                    "tx-pool service operation ended without mutation: {operational:?}"
                );
                Ok(())
            }
        }
    }

    pub(crate) fn config(&self) -> &TxPoolConfig {
        &self.config
    }

    /// Construct the authority, relay publisher and read-only reconciliation
    /// drain from one executor/configuration/snapshot tuple before any task
    /// starts. The exact executor identity fixes the parent-progress bound.
    pub(crate) fn prepare(
        handle: &Handle,
        config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
    ) -> Result<(AuthorityServiceBootstrap, AuthorityRelayDrain), AuthorityServiceStartError> {
        let consensus = snapshot.consensus();
        let (runtime, max_parents) = AuthorityRuntime::new_with_relay_parent_limit(
            handle,
            &config,
            consensus,
            Arc::clone(&snapshot),
        )
        .map_err(map_runtime_start_error)?;
        let (sink, receiver) =
            production_authority_relay_mailbox(DEFAULT_CHANNEL_SIZE, max_parents)
                .map_err(map_relay_start_error)?;
        let Some(scan_limit) = NonZeroUsize::new(RELAY_REBUILD_SCAN_ITEMS) else {
            return Err(AuthorityServiceStartError::RelayConfiguration);
        };
        let drain = AuthorityRelayDrain::new(receiver, runtime.relay_parent_reader(), scan_limit);
        Ok((
            AuthorityServiceBootstrap {
                config,
                runtime,
                relay_sink: sink,
            },
            drain,
        ))
    }

    /// Construct every capability before any worker starts. The topology
    /// claims the sole effect consumer before spawning its first task.
    pub(crate) async fn assemble(
        handle: &Handle,
        inputs: AuthorityServiceInputs,
    ) -> Result<AuthorityServiceAssembly, AuthorityServiceStartError> {
        let AuthorityServiceInputs {
            bootstrap,
            block_assembler,
            verification_cache,
            callbacks,
            network,
            persistence_writer,
            recent_reject,
            fee_estimator,
            chain_control_receiver,
            verification_control,
            cancel: parent_cancel,
        } = inputs;
        let AuthorityServiceBootstrap {
            config,
            runtime,
            relay_sink,
        } = bootstrap;
        let persistence_base = config.persisted_data.clone();
        let block_assembler = match block_assembler {
            Some(assembler) => Some(
                AuthorityBlockAssembler::new(runtime.clone(), assembler)
                    .await
                    .map_err(|_| AuthorityServiceStartError::TemplateConstruction)?,
            ),
            None => None,
        };
        let cancel = parent_cancel.child_token();
        let endpoints = AuthorityEffectEndpoints::new(
            network,
            relay_sink,
            Arc::new(callbacks),
            recent_reject.clone(),
        );
        let chunk_rx = verification_control.receiver();
        let topology = AuthorityTaskTopology::start(
            handle,
            runtime.clone(),
            Arc::clone(&verification_cache),
            verification_control,
            endpoints,
            block_assembler.clone(),
            cancel.clone(),
        )
        .map_err(map_topology_start_error)?;
        let service = Self {
            runtime,
            config: Arc::new(config),
            block_assembler,
            verification_cache,
            chunk_rx,
            cancel: cancel.clone(),
            persistence_writer,
            persistence_base,
            recent_reject,
            fee_estimator,
        };
        let control_service = service.clone();
        let control_cancel = cancel.child_token();
        let chain_control = handle.spawn(async move {
            run_ordered_chain_control_driver(
                control_service,
                chain_control_receiver,
                control_cancel,
            )
            .await
        });
        Ok(AuthorityServiceAssembly {
            service,
            generation: AuthorityGeneration {
                topology: Some(topology),
                chain_control: Some(chain_control),
                cancel,
                invalid: None,
            },
        })
    }

    pub(crate) async fn submit_remote(
        &self,
        tx: BoundedTransaction,
        declared_cycles: u64,
        peer: PeerIndex,
    ) -> Result<(), AuthorityServiceError> {
        let submissions = vec![(tx, declared_cycles)];
        let (completed, error) = self
            .submit_remote_batch(peer, submissions)
            .await
            .into_parts();
        match (completed, error) {
            (1, None) => Ok(()),
            (_, Some(error)) => Err(error),
            _ => Err(AuthorityServiceError::from(
                AuthorityFault::MembershipProjection,
            )),
        }
    }

    pub(crate) async fn submit_remote_batch(
        &self,
        peer: PeerIndex,
        submissions: Vec<(BoundedTransaction, u64)>,
    ) -> RemoteIngressBatchProgress {
        let bytes = retained_batch_bytes(submissions.iter().map(|(transaction, _)| transaction));
        if submissions.len() > ckb_constant::sync::MAX_RELAY_TXS_NUM_PER_BATCH
            || !matches!(bytes, Ok(bytes) if bytes
                <= ckb_constant::sync::MAX_RELAY_TXS_BYTES_PER_BATCH)
        {
            return RemoteIngressBatchProgress::failed(
                0,
                AuthorityServiceError::ResourceUnavailable,
            );
        }
        let expected_total = submissions.len();
        let consensus = self.runtime.paired_consensus();
        let mut submissions = VecDeque::from(submissions);
        let mut completed_total = 0usize;
        loop {
            let mut attempts =
                VecDeque::with_capacity(crate::constants::MAX_POOL_MUTATION_CANDIDATES);
            for _ in 0..crate::constants::MAX_POOL_MUTATION_CANDIDATES {
                let Some((tx, declared_cycles)) = submissions.pop_front() else {
                    break;
                };
                attempts.push_back(remote(tx, declared_cycles, peer, &consensus));
            }
            if attempts.is_empty() {
                return if completed_total == expected_total {
                    RemoteIngressBatchProgress::complete(completed_total)
                } else {
                    RemoteIngressBatchProgress::failed(
                        completed_total,
                        AuthorityServiceError::from(AuthorityFault::MembershipProjection),
                    )
                };
            }
            let expected_chunk = attempts.len();
            let (completed_chunk, error) = self
                .submit_retained_attempts(attempts)
                .await
                .into_checked_parts(expected_chunk);
            let Some(next_completed) = completed_total.checked_add(completed_chunk) else {
                return RemoteIngressBatchProgress::failed(
                    completed_total,
                    AuthorityServiceError::from(AuthorityFault::CounterExhausted),
                );
            };
            completed_total = next_completed;
            if let Some(error) = error {
                return RemoteIngressBatchProgress::failed(completed_total, error);
            }
        }
    }

    pub(crate) async fn submit_proposal_batch(
        &self,
        transactions: Vec<BoundedTransaction>,
    ) -> Result<(), AuthorityServiceError> {
        if transactions.len() > ckb_constant::sync::MAX_RELAY_TXS_NUM_PER_BATCH
            || (transactions.len() != 1
                && retained_batch_bytes(transactions.iter())?
                    > ckb_constant::sync::MAX_RELAY_TXS_BYTES_PER_BATCH)
        {
            return Err(AuthorityServiceError::ResourceUnavailable);
        }
        let consensus = self.runtime.paired_consensus();
        let mut transactions = transactions.into_iter();
        loop {
            let mut attempts =
                VecDeque::with_capacity(crate::constants::MAX_POOL_MUTATION_CANDIDATES);
            for _ in 0..crate::constants::MAX_POOL_MUTATION_CANDIDATES {
                let Some(tx) = transactions.next() else {
                    break;
                };
                attempts.push_back(proposal(tx, &consensus));
            }
            if attempts.is_empty() {
                return Ok(());
            }
            let expected = attempts.len();
            let (completed, error) = self.submit_retained_attempts(attempts).await.into_parts();
            if let Some(error) = error {
                return Err(error);
            }
            if completed != expected {
                return Err(AuthorityServiceError::from(
                    AuthorityFault::MembershipProjection,
                ));
            }
        }
    }

    async fn submit_retained_attempts(
        &self,
        mut attempts: VecDeque<RetainedIngressAttempt>,
    ) -> RemoteIngressBatchProgress {
        let mut completed = 0usize;
        while let Some(head) = attempts.pop_front() {
            let signal = self.runtime.effect_capacity_signal();
            let notified = signal.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();
            let batch = match RetainedAdmissionBatch::new(head, attempts) {
                Ok(batch) => batch,
                Err(error) => {
                    return RemoteIngressBatchProgress::failed(
                        completed,
                        map_retained_batch_error(error),
                    );
                }
            };
            match self.runtime.commit_retained_ingress_batch(batch) {
                Ok((consumed, remaining, post_commit_fault)) => {
                    let Some(next_completed) = completed.checked_add(consumed) else {
                        return RemoteIngressBatchProgress::failed(
                            completed,
                            AuthorityServiceError::from(AuthorityFault::CounterExhausted),
                        );
                    };
                    completed = next_completed;
                    attempts = remaining;
                    if let Some(fault) = post_commit_fault {
                        return RemoteIngressBatchProgress::failed(
                            completed,
                            AuthorityServiceError::from(fault),
                        );
                    }
                }
                Err(failure) => {
                    let (reason, batch) = failure.into_parts();
                    let error = match reason {
                        RetainedIngressBatchFailureReason::Plan(error) => error,
                        RetainedIngressBatchFailureReason::SharedContention => {
                            // Shared OCC loss is ordinary bounded admission
                            // backpressure. Remote and Proposal callers receive
                            // an explicit negative terminal; neither is banned,
                            // hidden for retry, or routed through the outer
                            // global write guard.
                            drop(batch);
                            return RemoteIngressBatchProgress::failed(
                                completed,
                                AuthorityServiceError::ResourceUnavailable,
                            );
                        }
                    };
                    let error = RetainedIngressBoundaryError::from_plan(error);
                    if matches!(
                        error,
                        RetainedIngressBoundaryError::Backpressure(
                            RetainedIngressBackpressure::EffectCapacity
                        )
                    ) {
                        attempts = batch.into_attempts();
                        if !wait_or_cancel(&self.cancel, notified.as_mut()).await {
                            return RemoteIngressBatchProgress::failed(
                                completed,
                                AuthorityServiceError::Cancelled,
                            );
                        }
                    } else {
                        return RemoteIngressBatchProgress::failed(
                            completed,
                            map_retained_batch_error(error),
                        );
                    }
                }
            }
        }
        RemoteIngressBatchProgress::complete(completed)
    }

    pub(crate) async fn submit_local(
        &self,
        transaction: BoundedTransaction,
    ) -> Result<Result<EntryCompleted, Reject>, AuthorityServiceError> {
        let started_at = Instant::now();
        let result = self.execute_direct(transaction, false).await;
        if matches!(result, Ok(Ok(_)))
            && let Some(metrics) = ckb_metrics::handle()
        {
            metrics
                .ckb_tx_pool_sync_process
                .observe(started_at.elapsed().as_secs_f64());
        }
        result
    }

    pub(crate) async fn test_accept(
        &self,
        transaction: BoundedTransaction,
    ) -> Result<Result<EntryCompleted, Reject>, AuthorityServiceError> {
        self.execute_direct(transaction, true).await
    }

    async fn execute_direct(
        &self,
        transaction: BoundedTransaction,
        test_accept: bool,
    ) -> Result<Result<EntryCompleted, Reject>, AuthorityServiceError> {
        let mut transaction = transaction.into_direct();
        loop {
            let Some(execution) = self.runtime.acquire_compute_execution(&self.cancel).await else {
                return Err(AuthorityServiceError::Cancelled);
            };
            let resolution = if test_accept {
                self.runtime
                    .resolve_test_accept_transaction(&transaction, execution)
            } else {
                self.runtime
                    .resolve_local_transaction(&transaction, execution)
            };
            let resolution = match resolution {
                Ok(resolution) => resolution,
                Err(DirectComputationError::StaleView) => continue,
                Err(DirectComputationError::ResourceUnavailable) => {
                    return Ok(Err(direct_pressure_reject()));
                }
                Err(DirectComputationError::InvalidEvidence) => {
                    return Err(AuthorityServiceError::from(
                        AuthorityFault::MembershipProjection,
                    ));
                }
            };
            let verified = match resolution {
                AuthorityDirectResolutionOutcome::Rejected(rejection) => {
                    let signal = self.runtime.effect_capacity_signal();
                    let notified = signal.notified();
                    tokio::pin!(notified);
                    let _ = notified.as_mut().enable();
                    let resource_wait = self.runtime.resource_capacity_wait_identity();
                    let resource_signal = resource_wait.terminal_signal();
                    let resource_notified = resource_signal.notified();
                    tokio::pin!(resource_notified);
                    let _ = resource_notified.as_mut().enable();
                    match self.runtime.settle_direct_transaction_rejection(rejection) {
                        Ok(AuthorityDirectRejectionExecution::Local(reason)) if !test_accept => {
                            return Ok(Err(reason.reject().clone()));
                        }
                        Ok(AuthorityDirectRejectionExecution::TestAccept(reason))
                            if test_accept =>
                        {
                            return Ok(Err(reason.reject().clone()));
                        }
                        Ok(
                            AuthorityDirectRejectionExecution::Local(_)
                            | AuthorityDirectRejectionExecution::TestAccept(_),
                        ) => {
                            return Err(AuthorityServiceError::from(
                                AuthorityFault::SchedulerProjection,
                            ));
                        }
                        Err(error) => match classify_direct_error(error) {
                            DirectErrorDisposition::Retry => continue,
                            DirectErrorDisposition::WaitEffect => {
                                if !wait_or_cancel(&self.cancel, notified.as_mut()).await {
                                    return Err(AuthorityServiceError::Cancelled);
                                }
                                continue;
                            }
                            DirectErrorDisposition::WaitResource(wait) => {
                                if !wait.same_bank(&resource_wait) {
                                    continue;
                                }
                                if !wait_or_cancel(&self.cancel, resource_notified.as_mut()).await {
                                    return Err(AuthorityServiceError::Cancelled);
                                }
                                continue;
                            }
                            DirectErrorDisposition::Reject(reject) => return Ok(Err(reject)),
                            DirectErrorDisposition::Service(error) => return Err(error),
                        },
                    }
                }
                AuthorityDirectResolutionOutcome::Verification(request) => {
                    let request = {
                        let cache = self.verification_cache.read().await;
                        request.bind_cache(&cache)
                    };
                    let mut command_rx = self.chunk_rx.clone();
                    match self
                        .runtime
                        .execute_direct_verification(request, &mut command_rx)
                        .await
                    {
                        Ok(outcome) => outcome,
                        Err(DirectComputationError::StaleView) => continue,
                        Err(DirectComputationError::ResourceUnavailable) => {
                            return Ok(Err(direct_pressure_reject()));
                        }
                        Err(DirectComputationError::InvalidEvidence) => {
                            return Err(AuthorityServiceError::from(
                                AuthorityFault::MembershipProjection,
                            ));
                        }
                    }
                }
            };
            match verified {
                AuthorityDirectVerificationOutcome::Rejected(rejection) => {
                    let signal = self.runtime.effect_capacity_signal();
                    let notified = signal.notified();
                    tokio::pin!(notified);
                    let _ = notified.as_mut().enable();
                    let resource_wait = self.runtime.resource_capacity_wait_identity();
                    let resource_signal = resource_wait.terminal_signal();
                    let resource_notified = resource_signal.notified();
                    tokio::pin!(resource_notified);
                    let _ = resource_notified.as_mut().enable();
                    match self.runtime.settle_direct_transaction_rejection(rejection) {
                        Ok(AuthorityDirectRejectionExecution::Local(reason)) if !test_accept => {
                            return Ok(Err(reason.reject().clone()));
                        }
                        Ok(AuthorityDirectRejectionExecution::TestAccept(reason))
                            if test_accept =>
                        {
                            return Ok(Err(reason.reject().clone()));
                        }
                        Ok(
                            AuthorityDirectRejectionExecution::Local(_)
                            | AuthorityDirectRejectionExecution::TestAccept(_),
                        ) => {
                            return Err(AuthorityServiceError::from(
                                AuthorityFault::SchedulerProjection,
                            ));
                        }
                        Err(error) => match classify_direct_error(error) {
                            DirectErrorDisposition::Retry => continue,
                            DirectErrorDisposition::WaitEffect => {
                                if !wait_or_cancel(&self.cancel, notified.as_mut()).await {
                                    return Err(AuthorityServiceError::Cancelled);
                                }
                                continue;
                            }
                            DirectErrorDisposition::WaitResource(wait) => {
                                if !wait.same_bank(&resource_wait) {
                                    continue;
                                }
                                if !wait_or_cancel(&self.cancel, resource_notified.as_mut()).await {
                                    return Err(AuthorityServiceError::Cancelled);
                                }
                                continue;
                            }
                            DirectErrorDisposition::Reject(reject) => return Ok(Err(reject)),
                            DirectErrorDisposition::Service(error) => return Err(error),
                        },
                    }
                }
                AuthorityDirectVerificationOutcome::Candidate(candidate) => {
                    let signal = self.runtime.effect_capacity_signal();
                    let notified = signal.notified();
                    tokio::pin!(notified);
                    let _ = notified.as_mut().enable();
                    let resource_wait = self.runtime.resource_capacity_wait_identity();
                    let resource_signal = resource_wait.terminal_signal();
                    let resource_notified = resource_signal.notified();
                    tokio::pin!(resource_notified);
                    let _ = resource_notified.as_mut().enable();
                    let outcome = match self.runtime.settle_verified_direct_admission(candidate) {
                        Ok(outcome) => outcome,
                        Err(error) => match classify_direct_error(error) {
                            DirectErrorDisposition::Retry => continue,
                            DirectErrorDisposition::WaitEffect => {
                                if !wait_or_cancel(&self.cancel, notified.as_mut()).await {
                                    return Err(AuthorityServiceError::Cancelled);
                                }
                                continue;
                            }
                            DirectErrorDisposition::WaitResource(wait) => {
                                if !wait.same_bank(&resource_wait) {
                                    continue;
                                }
                                if !wait_or_cancel(&self.cancel, resource_notified.as_mut()).await {
                                    return Err(AuthorityServiceError::Cancelled);
                                }
                                continue;
                            }
                            DirectErrorDisposition::Reject(reject) => return Ok(Err(reject)),
                            DirectErrorDisposition::Service(error) => return Err(error),
                        },
                    };
                    match (test_accept, outcome) {
                        (false, AuthorityDirectAdmissionExecution::Local(execution)) => {
                            let (outcome, cache_update) = execution.into_parts();
                            if let Some(update) = cache_update {
                                self.publish_cache_update(update).await;
                            }
                            match outcome {
                                AuthorityLocalAdmissionOutcome::Accepted(completed) => {
                                    return Ok(Ok(completed));
                                }
                                AuthorityLocalAdmissionOutcome::Duplicate(hash) => {
                                    return Ok(Err(Reject::Duplicated(hash.0)));
                                }
                                AuthorityLocalAdmissionOutcome::Rejected(reason) => {
                                    return Ok(Err(match reason {
                                        DirectAdmissionRejectionKind::Validation(reason) => {
                                            reason.reject().clone()
                                        }
                                        DirectAdmissionRejectionKind::Membership(reason) => {
                                            reason.into_public()
                                        }
                                    }));
                                }
                                AuthorityLocalAdmissionOutcome::Retry(retry) => {
                                    transaction = retry;
                                }
                            }
                        }
                        (true, AuthorityDirectAdmissionExecution::TestAccept(outcome)) => {
                            match outcome {
                                AuthorityTestAcceptOutcome::Accepted(completed) => {
                                    return Ok(Ok(completed));
                                }
                                AuthorityTestAcceptOutcome::Duplicate(hash) => {
                                    return Ok(Err(Reject::Duplicated(hash.0)));
                                }
                                AuthorityTestAcceptOutcome::RejectedValidation(reason) => {
                                    return Ok(Err(reason.reject().clone()));
                                }
                                AuthorityTestAcceptOutcome::RejectedMembership(reason) => {
                                    return Ok(Err(reason.into_public()));
                                }
                                AuthorityTestAcceptOutcome::Retry => {}
                            }
                        }
                        (false, AuthorityDirectAdmissionExecution::TestAccept(_))
                        | (true, AuthorityDirectAdmissionExecution::Local(_)) => {
                            return Err(AuthorityServiceError::from(
                                AuthorityFault::SchedulerProjection,
                            ));
                        }
                    }
                }
            }
        }
    }

    async fn publish_cache_update(&self, update: VerificationCacheUpdate) {
        self.verification_cache
            .write()
            .await
            .insert(update.into_proof());
    }

    pub(crate) async fn remove_local(
        &self,
        hash: &Byte32,
    ) -> Result<Result<bool, crate::service::LocalRemovalCompetingProgress>, AuthorityServiceError>
    {
        loop {
            let signal = self.runtime.effect_capacity_signal();
            let notified = signal.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();
            match self.runtime.remove_local_transaction(hash) {
                Ok(removed) => return Ok(Ok(removed)),
                Err(AuthorityAdministrationError::CompetingProgress) => {
                    return Ok(Err(crate::service::LocalRemovalCompetingProgress));
                }
                Err(AuthorityAdministrationError::EffectCapacity) => {
                    if !wait_or_cancel(&self.cancel, notified.as_mut()).await {
                        return Err(AuthorityServiceError::Cancelled);
                    }
                }
                Err(error) => return Err(map_administration_error(error)),
            }
        }
    }

    pub(crate) async fn clear_pipeline(&self) -> Result<(), AuthorityServiceError> {
        loop {
            let notified = self.runtime.effect_capacity_signal().notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();
            match self.runtime.clear_pipeline().await {
                Ok(()) => return Ok(()),
                Err(AuthorityAdministrationError::EffectCapacity) => {
                    if !wait_or_cancel(&self.cancel, notified.as_mut()).await {
                        return Err(AuthorityServiceError::Cancelled);
                    }
                }
                Err(error) => return Err(map_administration_error(error)),
            }
        }
    }

    pub(crate) async fn clear_pool(
        &self,
        snapshot: Arc<Snapshot>,
    ) -> Result<(), AuthorityServiceError> {
        loop {
            let notified = self.runtime.effect_capacity_signal().notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();
            match self.runtime.clear_pool(Arc::clone(&snapshot)).await {
                Ok(()) => return Ok(()),
                Err(AuthorityAdministrationError::EffectCapacity) => {
                    if !wait_or_cancel(&self.cancel, notified.as_mut()).await {
                        return Err(AuthorityServiceError::Cancelled);
                    }
                }
                Err(error) => return Err(map_administration_error(error)),
            }
        }
    }

    async fn commit_chain_update(
        &self,
        arguments: ChainReorgArgs,
    ) -> Result<CommittedChainUpdate, AuthorityChainUpdateError> {
        let committed = match arguments {
            ChainReorgArgs::ReplaceGeneration { snapshot } => self
                .runtime
                .apply_chain_generation_replacement(ChainGenerationReplacement::from_snapshot(
                    snapshot,
                ))
                .await
                .map_err(map_chain_generation_replacement_error)?,
            ChainReorgArgs::Detailed {
                detached_blocks,
                attached_blocks,
                snapshot,
            } => {
                let packaging = if self.block_assembler.is_some() {
                    CandidateUncleCollection::CollectCandidateUncles
                } else {
                    CandidateUncleCollection::SkipCandidateUncles
                };
                let request =
                    ChainUpdateRequest::new(detached_blocks, attached_blocks, snapshot, packaging);
                match request.prepare() {
                    Ok(command) => match self.runtime.apply_chain_update(command).await {
                        Ok(committed) => committed,
                        Err(failure) => {
                            let (error, returned) = failure.into_parts();
                            if let Some(fault) = map_chain_integrity(error) {
                                return Err(AuthorityChainUpdateError::integrity(fault));
                            }
                            if self.cancel.is_cancelled() {
                                return Err(AuthorityChainUpdateError::Cancelled);
                            }
                            self.runtime
                                .apply_chain_generation_replacement(
                                    returned.into_generation_replacement(),
                                )
                                .await
                                .map_err(map_chain_generation_replacement_error)?
                        }
                    },
                    Err(failure) => {
                        let (error, returned) = failure.into_parts();
                        if let Some(fault) = map_chain_integrity(error) {
                            return Err(AuthorityChainUpdateError::integrity(fault));
                        }
                        if self.cancel.is_cancelled() {
                            return Err(AuthorityChainUpdateError::Cancelled);
                        }
                        self.runtime
                            .apply_chain_generation_replacement(
                                returned.into_generation_replacement(),
                            )
                            .await
                            .map_err(map_chain_generation_replacement_error)?
                    }
                }
            }
        };
        Ok(committed)
    }

    #[must_use = "a committed chain fault must reach generation supervision after observers"]
    fn publish_chain_observers(&self, committed: CommittedChainUpdate) -> Option<AuthorityFault> {
        // Fee estimation is a derived observer of the committed chain cut.
        // It must never run during preparation (a retried or rejected command
        // is not chain history), and candidate-uncle publication cannot veto
        // this independent post-commit projection.
        for block in &committed.attached_blocks {
            self.fee_estimator.commit_block(block);
        }
        if let Some(assembler) = &self.block_assembler {
            for uncle in committed.candidate_uncles {
                observe_candidate_uncle(assembler, uncle);
            }
        }
        drop(committed.snapshot);
        committed.post_commit_fault
    }

    pub(crate) fn receive_candidate_uncle(
        &self,
        uncle: crate::block_assembler::BoundedCandidateUncle,
    ) {
        if let Some(assembler) = &self.block_assembler {
            observe_candidate_uncle(assembler, uncle);
        }
    }

    pub(crate) async fn block_template(
        &self,
    ) -> Result<ckb_jsonrpc_types::BlockTemplate, AuthorityServiceError> {
        match &self.block_assembler {
            Some(assembler) => assembler
                .current_template(&self.cancel)
                .await
                .map_err(map_template_availability),
            None => Err(AuthorityServiceError::BlockAssemblerDisabled),
        }
    }

    pub(crate) fn transaction_lookup(
        &self,
        hash: &Byte32,
    ) -> Result<AuthorityTransactionLookup, AuthorityServiceError> {
        self.runtime
            .transaction_lookup(hash)
            .map_err(map_query_error)
    }

    pub(crate) fn transaction_status_lookup(
        &self,
        hash: &Byte32,
    ) -> AuthorityTransactionStatusLookup {
        self.runtime.transaction_status_lookup(hash)
    }

    pub(crate) async fn pool_summary(&self) -> Result<AuthorityPoolSummary, AuthorityServiceError> {
        self.runtime.pool_summary().await.map_err(map_query_error)
    }

    pub(crate) fn filter_fresh_proposals(
        &self,
        proposals: Vec<ProposalShortId>,
    ) -> Result<Vec<ProposalShortId>, AuthorityServiceError> {
        self.runtime
            .filter_fresh_proposals(proposals)
            .map_err(map_query_error)
    }

    pub(crate) fn compact_block_receipt(
        &self,
        proposals: Vec<ProposalShortId>,
    ) -> Result<CompactBlockReadReceipt, AuthorityServiceError> {
        self.runtime
            .capture_compact_block(proposals)
            .map_err(map_query_error)
    }

    pub(crate) fn compact_transactions(
        &self,
        proposals: Vec<ProposalShortId>,
    ) -> Result<HashMap<ProposalShortId, TransactionView>, AuthorityServiceError> {
        self.compact_block_receipt(proposals)?
            .resolve()
            .map_err(map_query_error)
    }

    pub(crate) fn accepted_with_cycles(
        &self,
        tx_hashes: Vec<ckb_types::packed::Byte32>,
    ) -> Result<AcceptedTransactionsWithCycles, AuthorityServiceError> {
        self.runtime
            .accepted_with_cycles(tx_hashes)
            .map_err(map_query_error)
    }

    pub(crate) async fn pool_ids(
        &self,
    ) -> Result<ckb_types::core::tx_pool::TxPoolIds, AuthorityServiceError> {
        self.runtime.pool_ids().await.map_err(map_query_error)
    }

    pub(crate) async fn all_entry_info(
        &self,
    ) -> Result<ckb_types::core::tx_pool::TxPoolEntryInfo, AuthorityServiceError> {
        self.runtime.all_entry_info().await.map_err(map_query_error)
    }

    pub(crate) async fn pool_detail(
        &self,
        hash: &Byte32,
    ) -> Result<Option<ckb_types::core::tx_pool::PoolTxDetailInfo>, AuthorityServiceError> {
        self.runtime
            .pool_detail(hash)
            .await
            .map_err(map_query_error)
    }

    pub(crate) fn live_cell_receipt(&self, out_point: OutPoint) -> LiveCellReadReceipt {
        self.runtime.live_cell_receipt(out_point)
    }

    pub(crate) fn persistence_receipt(&self) -> Result<PersistenceReceipt, AuthorityServiceError> {
        self.runtime.persistence_receipt().map_err(map_query_error)
    }

    /// Persist one coherent authority read cut without retaining its guard
    /// across sorting or file I/O. Acquiring the unique writer before capture
    /// bounds concurrent save requests to one owned pool snapshot.
    pub(crate) async fn save_pool(&self) -> Result<(), AuthorityPersistenceError> {
        let writer = self.persistence_writer.acquire().await;
        let receipt = self
            .persistence_receipt()
            .map_err(AuthorityPersistenceError::Snapshot)?;
        let parent_first = receipt
            .into_parent_first()
            .map_err(map_query_error)
            .map_err(AuthorityPersistenceError::Snapshot)?;
        let (accepted, recovery) = parent_first.into_transactions();
        let mut accepted_transactions = Vec::with_capacity(accepted.len());
        accepted_transactions.extend(accepted.into_iter().map(Arc::unwrap_or_clone));
        let mut recovery_transactions = Vec::with_capacity(recovery.len());
        recovery_transactions.extend(recovery.into_iter().map(Arc::unwrap_or_clone));
        let snapshot = crate::persisted::PersistenceSnapshot {
            accepted: accepted_transactions,
            recovery: recovery_transactions,
        };
        let base = self.persistence_base.clone();
        match tokio::task::spawn_blocking(move || writer.write(&base, snapshot)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(AuthorityPersistenceError::Write(error)),
            Err(error) => Err(AuthorityPersistenceError::Join(error)),
        }
    }

    /// Replay persisted bytes only after the effect publisher and compute
    /// topology are live. Every item re-enters the ordinary Local validation
    /// path; the file carries no trusted ownership or verification evidence.
    pub(crate) async fn replay_persisted(
        &self,
        snapshot: crate::persisted::PersistenceSnapshot,
    ) -> Result<AuthorityPersistenceReplay, AuthorityPersistenceError> {
        let mut transactions = snapshot.into_transactions();
        crate::dependency_sort::sort_transactions(&mut transactions)
            .map_err(AuthorityPersistenceError::Sort)?;
        let mut replay = AuthorityPersistenceReplay::default();
        for transaction in transactions {
            let transaction = match BoundedTransaction::try_new(transaction) {
                Ok(transaction) => transaction,
                Err(super::ingress::BoundedTransactionError::TooLarge { .. }) => {
                    replay.stale = replay
                        .stale
                        .checked_add(1)
                        .ok_or(AuthorityPersistenceError::Counter)?;
                    continue;
                }
                Err(super::ingress::BoundedTransactionError::Allocation) => {
                    return Err(AuthorityPersistenceError::Replay(
                        AuthorityServiceError::ResourceUnavailable,
                    ));
                }
            };
            match self
                .submit_local(transaction)
                .await
                .map_err(AuthorityPersistenceError::Replay)?
            {
                Ok(_) => {
                    replay.loaded = replay
                        .loaded
                        .checked_add(1)
                        .ok_or(AuthorityPersistenceError::Counter)?;
                }
                Err(_) => {
                    replay.stale = replay
                        .stale
                        .checked_add(1)
                        .ok_or(AuthorityPersistenceError::Counter)?;
                }
            }
        }
        Ok(replay)
    }

    pub(crate) async fn fee_estimate_receipt(
        &self,
    ) -> Result<FeeEstimateReadReceipt, AuthorityServiceError> {
        self.runtime
            .fee_estimate_receipt()
            .await
            .map_err(map_query_error)
    }

    pub(crate) fn update_ibd_state(&self, in_ibd: bool) {
        self.fee_estimator.update_ibd_state(in_ibd);
    }

    pub(crate) fn total_recent_reject_num(&self) -> Option<u64> {
        self.recent_reject
            .as_ref()
            .map(|recent| recent.get_estimate_total_keys_num())
    }

    pub(crate) fn recent_reject_record(
        &self,
        hash: &Byte32,
    ) -> Result<Option<String>, AuthorityDerivedError> {
        if let Some(record) = self.pending_recent_reject(hash)? {
            return Ok(Some(record));
        }
        match &self.recent_reject {
            Some(recent) => recent.get(hash).map_err(AuthorityDerivedError::External),
            None => Ok(None),
        }
    }

    pub(crate) async fn estimate_fee_rate(
        &self,
        estimate_mode: EstimateMode,
        enable_fallback: bool,
    ) -> Result<FeeRate, AuthorityDerivedError> {
        let entries = self
            .all_entry_info()
            .await
            .map_err(AuthorityDerivedError::Authority)?;
        match self.fee_estimator.estimate_fee_rate(estimate_mode, entries) {
            Ok(rate) => Ok(rate),
            Err(error) if !enable_fallback => Err(AuthorityDerivedError::External(error.into())),
            Err(_) => {
                let target = FeeEstimator::target_blocks_for_estimate_mode(estimate_mode);
                self.fee_estimate_receipt()
                    .await
                    .map_err(AuthorityDerivedError::Authority)?
                    .estimate(target)
                    .map_err(|error| {
                        AuthorityDerivedError::External(
                            ckb_error::OtherError::new(format!(
                                "tx-pool fallback fee estimate failed: {error:?}"
                            ))
                            .into(),
                        )
                    })
            }
        }
    }

    pub(crate) fn pending_recent_reject(
        &self,
        hash: &Byte32,
    ) -> Result<Option<String>, AuthorityDerivedError> {
        self.runtime
            .pending_recent_reject(hash)
            .map_err(map_recent_reject_read_error)
    }

    /// Inject already-resolved test instrumentation one entry at a time.
    /// Each successful item is one complete authority Apply, preserving the
    /// historical partial-success contract if a later fixture is rejected.
    #[cfg(any(test, feature = "internal"))]
    pub(crate) async fn plug_entry(
        &self,
        entries: Vec<TxEntry>,
        target: PlugTarget,
    ) -> Result<(), Reject> {
        let status = match target {
            PlugTarget::Pending => AcceptedStatus::Pending,
            PlugTarget::Proposed => AcceptedStatus::Proposed,
        };
        for entry in entries {
            loop {
                match self.runtime.plug_internal_entry(&entry, status) {
                    Ok(
                        AuthorityInternalPlugOutcome::Inserted
                        | AuthorityInternalPlugOutcome::Duplicate,
                    ) => break,
                    Err(AuthorityInternalPlugError::Stale) => {
                        if self.cancel.is_cancelled() {
                            return Err(Reject::Internal(
                                "tx-pool authority generation is shutting down".to_owned(),
                            ));
                        }
                        tokio::task::yield_now().await;
                    }
                    Err(error) => return Err(internal_plug_reject(error)),
                }
            }
        }
        Ok(())
    }

    #[cfg(any(test, feature = "internal"))]
    pub(crate) fn package_transactions(
        &self,
        bytes_limit: Option<u64>,
    ) -> Result<Vec<TxEntry>, AuthorityServiceError> {
        let input = self
            .runtime
            .template_input()
            .map_err(map_template_read_error)?;
        let max_bytes = bytes_limit.unwrap_or(input.snapshot().consensus().max_block_bytes());
        let max_bytes =
            usize::try_from(max_bytes).map_err(|_| AuthorityServiceError::ResourceUnavailable)?;
        input
            .selection()
            .pack_transactions(TemplatePackingLimits::new(
                max_bytes,
                input.snapshot().consensus().max_block_cycles(),
            ))
            .map(|packed| packed.into_tx_entries())
            .map_err(map_template_read_error)
    }
}

/// Candidate uncles are bounded, rebuildable template input. Failure to
/// advance their private source counter must not veto an already-committed
/// chain Apply or invalidate the independent transaction authority.
fn observe_candidate_uncle(
    assembler: &AuthorityBlockAssembler,
    uncle: crate::block_assembler::BoundedCandidateUncle,
) {
    record_candidate_uncle_observation(assembler.receive_candidate_uncle(uncle));
}

pub(super) fn record_candidate_uncle_observation(
    result: Result<bool, AuthorityTemplateDriverFault>,
) {
    if let Err(error) = result {
        ckb_logger::error!("tx-pool candidate-uncle observer degraded: {error:?}");
    }
}

pub(super) fn map_recent_reject_read_error(
    error: AuthorityRecentRejectReadError,
) -> AuthorityDerivedError {
    match error {
        AuthorityRecentRejectReadError::Projection => AuthorityDerivedError::Authority(
            AuthorityServiceError::from(AuthorityFault::EffectProjection),
        ),
        AuthorityRecentRejectReadError::Encoding(error) => {
            AuthorityDerivedError::External(error.into())
        }
    }
}

#[cfg(any(test, feature = "internal"))]
fn internal_plug_reject(error: AuthorityInternalPlugError) -> Reject {
    match error {
        AuthorityInternalPlugError::Stale => {
            Reject::Internal("stale internal tx-pool fixture escaped its retry boundary".to_owned())
        }
        AuthorityInternalPlugError::WouldDisplace => Reject::RBFRejected(
            "internal tx-pool fixture cannot displace an accepted owner".to_owned(),
        ),
        AuthorityInternalPlugError::Rejected(reason) => reason.into_public(),
        AuthorityInternalPlugError::Capacity | AuthorityInternalPlugError::ProposalCollision => {
            direct_pressure_reject()
        }
        AuthorityInternalPlugError::LifecycleClosed => {
            Reject::Internal("tx-pool authority lifecycle is closed".to_owned())
        }
        AuthorityInternalPlugError::Fault(fault) => Reject::Internal(format!(
            "internal tx-pool fixture violated the authority contract: {fault:?}"
        )),
    }
}

fn map_runtime_start_error(_error: RuntimeConfigError) -> AuthorityServiceStartError {
    AuthorityServiceStartError::RuntimeConfiguration
}

fn map_relay_start_error(_error: RelayMailboxConfigError) -> AuthorityServiceStartError {
    AuthorityServiceStartError::RelayConfiguration
}

fn retained_batch_bytes<'a>(
    transactions: impl IntoIterator<Item = &'a BoundedTransaction>,
) -> Result<usize, AuthorityServiceError> {
    transactions
        .into_iter()
        .try_fold(0usize, |total, transaction| {
            total
                .checked_add(transaction.payload_bytes())
                .ok_or(AuthorityServiceError::ResourceUnavailable)
        })
}

fn map_topology_start_error(error: AuthorityTopologyStartError) -> AuthorityServiceStartError {
    match error {
        AuthorityTopologyStartError::Cancelled => AuthorityServiceStartError::Cancelled,
        AuthorityTopologyStartError::EffectPublisherClaimed => {
            AuthorityServiceStartError::EffectPublisherClaimed
        }
        AuthorityTopologyStartError::Worker(_) => AuthorityServiceStartError::WorkerAllocation,
    }
}

fn map_retained_batch_error(error: RetainedIngressBoundaryError) -> AuthorityServiceError {
    match error {
        RetainedIngressBoundaryError::InvalidEvidence => {
            AuthorityServiceError::from(AuthorityFault::MembershipProjection)
        }
        RetainedIngressBoundaryError::ResourceUnavailable
        | RetainedIngressBoundaryError::Backpressure(
            RetainedIngressBackpressure::TotalResources
            | RetainedIngressBackpressure::RemoteResources
            | RetainedIngressBackpressure::PeerResources
            | RetainedIngressBackpressure::ComputeResources
            | RetainedIngressBackpressure::ProposalCollision,
        ) => AuthorityServiceError::ResourceUnavailable,
        RetainedIngressBoundaryError::Backpressure(RetainedIngressBackpressure::EffectCapacity) => {
            AuthorityServiceError::EffectCapacity
        }
        RetainedIngressBoundaryError::LifecycleClosed => AuthorityServiceError::LifecycleClosed,
        RetainedIngressBoundaryError::Fault(fault) => AuthorityServiceError::from(fault),
    }
}

enum DirectErrorDisposition {
    Retry,
    WaitEffect,
    WaitResource(ResourceCapacityWaitIdentity),
    Reject(Reject),
    Service(AuthorityServiceError),
}

fn classify_direct_error(error: AuthorityDirectAdmissionError) -> DirectErrorDisposition {
    match error {
        AuthorityDirectAdmissionError::Stale => DirectErrorDisposition::Retry,
        AuthorityDirectAdmissionError::ResourceContended(wait) => {
            DirectErrorDisposition::WaitResource(wait)
        }
        AuthorityDirectAdmissionError::ProposalCollision => {
            DirectErrorDisposition::Reject(direct_pressure_reject())
        }
        AuthorityDirectAdmissionError::EffectCapacity => DirectErrorDisposition::WaitEffect,
        AuthorityDirectAdmissionError::LifecycleClosed => {
            DirectErrorDisposition::Service(AuthorityServiceError::LifecycleClosed)
        }
        AuthorityDirectAdmissionError::Fault(fault) => {
            DirectErrorDisposition::Service(AuthorityServiceError::from(fault))
        }
    }
}

fn direct_pressure_reject() -> Reject {
    Reject::Full("tx-pool cannot admit the transaction under current resource limits".to_owned())
}

async fn wait_or_cancel(
    cancel: &CancellationToken,
    mut notified: Pin<&mut tokio::sync::futures::Notified<'_>>,
) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = notified.as_mut() => true,
    }
}

async fn run_ordered_chain_control_driver(
    service: AuthorityService,
    mut receiver: mpsc::Receiver<ChainControl>,
    cancel: CancellationToken,
) -> Result<(), AuthorityGenerationInvalidity> {
    loop {
        let command = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            command = receiver.recv() => command,
        };
        let Some(command) = command else {
            return Ok(());
        };
        match command {
            ChainControl::Reconcile(Request {
                responder,
                arguments,
            }) => {
                match service.commit_chain_update(arguments).await {
                    Ok(committed) => {
                        // This response is the exact chain-to-authority
                        // visibility barrier.  Rebuildable fee/template
                        // observers remain outside the coupled completion cut.
                        respond(responder, (), "chain_reconcile_apply");
                        if let Some(fault) = service.publish_chain_observers(committed) {
                            return Err(AuthorityGenerationInvalidity::from_integrity(
                                AuthorityIntegrityFault::from(fault),
                            ));
                        }
                    }
                    Err(AuthorityChainUpdateError::Cancelled) => return Ok(()),
                    Err(AuthorityChainUpdateError::Integrity(invalidity)) => {
                        return Err(invalidity);
                    }
                }
            }
            ChainControl::ClearPool(command) => {
                let (
                    admission,
                    Request {
                        responder,
                        arguments,
                    },
                ) = command.into_parts();
                let result = service.clear_pool(arguments).await;
                drop(admission);
                settle_ordered_administration(responder, result, "clear_pool")?;
            }
            ChainControl::ClearPipeline(command) => {
                let (admission, Request { responder, .. }) = command.into_parts();
                let result = service.clear_pipeline().await;
                drop(admission);
                settle_ordered_administration(responder, result, "clear_pipeline")?;
            }
        }
    }
}

fn settle_ordered_administration<S>(
    responder: S,
    result: Result<(), AuthorityServiceError>,
    operation: &'static str,
) -> Result<(), AuthorityGenerationInvalidity>
where
    S: OneshotSender<()>,
{
    match result {
        Ok(()) => {
            respond(responder, (), operation);
            Ok(())
        }
        Err(error) => {
            drop(responder);
            AuthorityService::settle_operation_error(error)
        }
    }
}

fn map_administration_error(error: AuthorityAdministrationError) -> AuthorityServiceError {
    match error {
        AuthorityAdministrationError::EffectCapacity => AuthorityServiceError::EffectCapacity,
        AuthorityAdministrationError::LifecycleClosed => AuthorityServiceError::LifecycleClosed,
        AuthorityAdministrationError::CompetingProgress => AuthorityServiceError::CompetingProgress,
        AuthorityAdministrationError::Fault(fault) => AuthorityServiceError::from(fault),
    }
}

pub(super) fn map_chain_integrity(error: ChainBoundaryError) -> Option<AuthorityIntegrityFault> {
    match error {
        ChainBoundaryError::Allocation => None,
        ChainBoundaryError::LifecycleClosed => Some(AuthorityIntegrityFault::EffectLifecycleClosed),
        ChainBoundaryError::CounterExhausted => Some(AuthorityIntegrityFault::from(
            AuthorityFault::CounterExhausted,
        )),
        ChainBoundaryError::InvalidFacts | ChainBoundaryError::InvalidSnapshotEvidence => {
            Some(AuthorityIntegrityFault::InvalidChainEvidence)
        }
        ChainBoundaryError::Fault(fault) => Some(AuthorityIntegrityFault::from(fault)),
    }
}

fn map_chain_generation_replacement_error(
    error: AuthorityGenerationReplacementError,
) -> AuthorityChainUpdateError {
    match error {
        AuthorityGenerationReplacementError::LifecycleClosed => {
            AuthorityChainUpdateError::integrity(AuthorityIntegrityFault::EffectLifecycleClosed)
        }
        AuthorityGenerationReplacementError::Fault(AuthorityFault::CounterExhausted) => {
            AuthorityChainUpdateError::integrity(AuthorityIntegrityFault::from(
                AuthorityFault::CounterExhausted,
            ))
        }
        AuthorityGenerationReplacementError::Fault(fault) => {
            AuthorityChainUpdateError::integrity(AuthorityIntegrityFault::from(fault))
        }
    }
}

fn map_query_error(error: AuthorityQueryError) -> AuthorityServiceError {
    match error {
        AuthorityQueryError::Allocation => AuthorityServiceError::ResourceUnavailable,
        AuthorityQueryError::Arithmetic => {
            AuthorityServiceError::from(AuthorityFault::ResourceProjection)
        }
        AuthorityQueryError::Projection
        | AuthorityQueryError::AcceptedCycle
        | AuthorityQueryError::RecoveryCycle => {
            AuthorityServiceError::from(AuthorityFault::MembershipProjection)
        }
    }
}

#[cfg(any(test, feature = "internal"))]
fn map_template_read_error(error: TemplateReadError) -> AuthorityServiceError {
    match error {
        TemplateReadError::Arithmetic => {
            AuthorityServiceError::from(AuthorityFault::ResourceProjection)
        }
        TemplateReadError::Projection | TemplateReadError::CausalCycle => {
            AuthorityServiceError::from(AuthorityFault::MembershipProjection)
        }
    }
}

fn map_template_availability(error: AuthorityTemplateReadFailure) -> AuthorityServiceError {
    match error {
        AuthorityTemplateReadFailure::Cancelled => AuthorityServiceError::Cancelled,
        AuthorityTemplateReadFailure::Unavailable => AuthorityServiceError::TemplateUnavailable,
    }
}
