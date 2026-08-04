//! Narrow service boundary for the unified authority kernel.
//!
//! This module is the only place where production-shaped service operations
//! may call the authority's private capabilities. It owns no transaction
//! facts: all mutation remains in [`AuthorityRuntime`], while this adapter
//! performs exhaustive conversion to closed service outcomes after Plan/Apply.

pub(crate) use super::relay::AuthorityRelaySink;
use super::{
    chain_boundary::{ChainBoundaryError, ChainPackaging, ChainUpdateRequest},
    effect::ParentTransactionRequest,
    ingress::{
        RemoteIngressPressure, RetainedIngressBackpressure, RetainedIngressBoundaryError,
        RetainedIngressCommit,
    },
    plan::AuthorityFault,
    publisher::AuthorityEffectEndpoints,
    query::{
        AuthorityPoolSummary, AuthorityQueryError, AuthorityTransactionLookup,
        CompactBlockReadReceipt, FeeEstimateReadReceipt, LiveCellReadReceipt, PersistenceReceipt,
    },
    read::{RelayParentRebuildCursor, RelayParentRebuildError},
    relay::{
        AuthorityRelayReceiver, RelayMailboxConfigError, RelayParentProjectionError,
        production_authority_relay_mailbox, project_parent_request,
    },
    resolver::{DirectComputationError, VerificationCacheUpdate},
    runtime::{
        AuthorityAdministrationError, AuthorityDirectAdmissionError,
        AuthorityDirectAdmissionExecution, AuthorityDirectRejectionExecution,
        AuthorityDirectResolutionOutcome, AuthorityDirectVerificationOutcome,
        AuthorityLocalAdmissionOutcome, AuthorityRelayParentReader, AuthorityRuntime,
        AuthorityTestAcceptOutcome, DirectAdmissionRejectionKind, RuntimeConfigError,
    },
    template::TemplateReadError,
    template_driver::AuthorityBlockAssembler,
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
};
#[cfg(any(test, feature = "internal"))]
use crate::{PlugTarget, component::entry::TxEntry};
use crate::{
    block_assembler::BlockAssembler,
    callback::Callbacks,
    dependency_sort::DependencySortError,
    error::Reject,
    network::TxPoolNetworkHandle,
    service::{ChainReorgArgs, DEFAULT_CHANNEL_SIZE, TxVerificationResult},
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
    core::{EstimateMode, FeeRate, TransactionView, UncleBlockView, tx_pool::EntryCompleted},
    packed::{Byte32, OutPoint, ProposalShortId},
};
use ckb_util::Mutex;
use ckb_verification::cache::TxVerificationCache;
use std::{
    collections::HashMap,
    num::NonZeroUsize,
    path::PathBuf,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityServiceError {
    Cancelled,
    BlockAssemblerDisabled,
    ResourceUnavailable,
    EffectCapacity,
    LifecycleClosed,
    InvalidChainEvidence,
    CounterExhausted,
    Projection(AuthorityProjectionFault),
}

/// Move-only proof that a service error is a structural contradiction and
/// therefore makes this authority generation ineligible for persistence.
/// Operational outcomes cannot construct this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuthorityGenerationInvalidity(AuthorityServiceError);

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityProjectionFault {
    Counter,
    Resource,
    Membership,
    Index,
    Scheduler,
    Dependency,
    Effect,
}

impl From<AuthorityFault> for AuthorityProjectionFault {
    fn from(fault: AuthorityFault) -> Self {
        match fault {
            AuthorityFault::CounterExhausted => Self::Counter,
            AuthorityFault::ResourceProjection => Self::Resource,
            AuthorityFault::MembershipProjection => Self::Membership,
            AuthorityFault::IndexProjection => Self::Index,
            AuthorityFault::SchedulerProjection => Self::Scheduler,
            AuthorityFault::DependencyProjection => Self::Dependency,
            AuthorityFault::EffectProjection => Self::Effect,
        }
    }
}

/// Retained ingress has no synchronous public rejection result. This closed
/// disposition nevertheless forces the service cutover to account for every
/// no-owner outcome, including relayer filter release under pressure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityIngressDisposition {
    Retained,
    AcceptedDuplicate,
    RemoteReleased,
    ProposalUnchanged,
    Rejected,
    Pressure(AuthorityIngressPressure),
    PeerRevoked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityIngressPressure {
    TotalResources,
    RemoteResources,
    PeerResources,
    ComputeResources,
    EffectCapacity,
    ProposalCollision,
    Allocation,
}

/// Immutable construction inputs. This value exists only until the runtime,
/// derived endpoints and task topology have all been constructed.
pub(crate) struct AuthorityServiceInputs {
    pub(crate) bootstrap: AuthorityServiceBootstrap,
    pub(crate) block_assembler: Option<BlockAssembler>,
    pub(crate) verification_cache: Arc<RwLock<TxVerificationCache>>,
    pub(crate) callbacks: Callbacks,
    pub(crate) network: TxPoolNetworkHandle,
    pub(crate) persistence_writer: Arc<crate::persisted::PersistenceWriter>,
    pub(crate) recent_reject: Option<Arc<crate::component::recent_reject::RecentReject>>,
    pub(crate) fee_estimator: FeeEstimator,
    pub(crate) reorg_receiver: mpsc::Receiver<crate::service::Notify<ChainReorgArgs>>,
    pub(crate) chunk_rx: watch::Receiver<ChunkCommand>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthorityRelayRebuildFailure {
    Page(RelayParentRebuildError),
    Projection(RelayParentProjectionError),
}

struct AuthorityRelayRebuildState {
    active: bool,
    cursor: Option<RelayParentRebuildCursor>,
    pending: Vec<ParentTransactionRequest>,
    reported_failure: Option<AuthorityRelayRebuildFailure>,
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

    fn report_once(&mut self, failure: AuthorityRelayRebuildFailure) {
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
                match project_parent_request(&request) {
                    Ok(result) => {
                        rebuild.reported_failure = None;
                        return Some(result);
                    }
                    Err(error) => {
                        rebuild.pending.push(request);
                        rebuild.report_once(AuthorityRelayRebuildFailure::Projection(error));
                        return None;
                    }
                }
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
                    rebuild.report_once(AuthorityRelayRebuildFailure::Page(error));
                    return None;
                }
            }
        }
        None
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
    reorg: Option<tokio::task::JoinHandle<Result<(), AuthorityGenerationInvalidity>>>,
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
    Reorg(
        #[expect(
            dead_code,
            reason = "the reorg-invalid capability is retained until shutdown and rendered in the operational fault log"
        )]
        AuthorityGenerationInvalidity,
    ),
    ReorgJoin(tokio::task::JoinError),
    ReorgTimeout,
}

impl AuthorityGeneration {
    pub(crate) fn invalidate(&mut self, fault: AuthorityGenerationInvalidity) {
        self.retain_invalid(AuthorityServiceGenerationFault::Service(fault));
    }

    pub(crate) async fn next_event(&mut self) -> AuthorityGenerationEvent {
        if self.invalid.is_some() {
            return AuthorityGenerationEvent::GenerationInvalid;
        }
        let event = match (self.topology.as_mut(), self.reorg.as_mut()) {
            (Some(topology), Some(reorg)) => {
                tokio::select! {
                    event = topology.next_event() => GenerationBoundaryEvent::Topology(event),
                    result = reorg => GenerationBoundaryEvent::Reorg(result),
                }
            }
            (Some(topology), None) => {
                GenerationBoundaryEvent::Topology(topology.next_event().await)
            }
            (None, Some(reorg)) => GenerationBoundaryEvent::Reorg(reorg.await),
            (None, None) => return AuthorityGenerationEvent::ShutdownRequested,
        };
        match event {
            GenerationBoundaryEvent::Topology(event) => self.classify_topology_event(event),
            GenerationBoundaryEvent::Reorg(result) => {
                self.reorg = None;
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
                        self.retain_invalid(AuthorityServiceGenerationFault::Reorg(fault));
                        AuthorityGenerationEvent::GenerationInvalid
                    }
                    Err(error) => {
                        self.retain_invalid(AuthorityServiceGenerationFault::ReorgJoin(error));
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

    pub(crate) async fn shutdown(mut self, timeout: Duration) -> AuthorityShutdownOutcome {
        self.cancel.cancel();
        if let Some(mut reorg) = self.reorg.take() {
            match tokio::time::timeout(timeout, &mut reorg).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(fault))) => {
                    self.retain_invalid(AuthorityServiceGenerationFault::Reorg(fault));
                }
                Ok(Err(error)) => {
                    self.retain_invalid(AuthorityServiceGenerationFault::ReorgJoin(error));
                }
                Err(_) => {
                    reorg.abort();
                    let _ = reorg.await;
                    self.retain_invalid(AuthorityServiceGenerationFault::ReorgTimeout);
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
                topology.invalidate_generation(fault)
            }
            Some(fault) => {
                ckb_logger::error!(
                    "tx-pool ordered reorg generation failed; persistence is forbidden: {fault:?}"
                );
                drop(topology);
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
        AuthorityServiceGenerationFault::Service(_) | AuthorityServiceGenerationFault::Reorg(_) => {
            crate::metrics::FailureBoundary::TypedFault
        }
        AuthorityServiceGenerationFault::ReorgJoin(error) if error.is_panic() => {
            crate::metrics::FailureBoundary::HandlerUnwind
        }
        AuthorityServiceGenerationFault::ReorgJoin(_)
        | AuthorityServiceGenerationFault::ReorgTimeout => {
            crate::metrics::FailureBoundary::WorkerExit
        }
    }
}

pub(super) fn authority_failure_boundary(
    fault: &AuthorityGenerationFault,
) -> crate::metrics::FailureBoundary {
    match fault {
        AuthorityGenerationFault::Worker {
            fault: AuthorityWorkerFaultKind::Authority(_) | AuthorityWorkerFaultKind::Settlement(_),
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
    Reorg(Result<Result<(), AuthorityGenerationInvalidity>, tokio::task::JoinError>),
}

impl AuthorityService {
    /// Classify one closed service error at the compatibility boundary.
    /// Only structural contradictions can yield the linear invalidity proof.
    pub(crate) fn settle_operation_error(
        error: AuthorityServiceError,
    ) -> Result<(), AuthorityGenerationInvalidity> {
        if matches!(
            error,
            AuthorityServiceError::InvalidChainEvidence
                | AuthorityServiceError::CounterExhausted
                | AuthorityServiceError::Projection(_)
        ) {
            Err(AuthorityGenerationInvalidity(error))
        } else {
            ckb_logger::debug!("tx-pool service operation ended without mutation: {error:?}");
            Ok(())
        }
    }

    pub(crate) fn config(&self) -> &TxPoolConfig {
        &self.config
    }

    /// Construct the authority, relay publisher and read-only reconciliation
    /// drain from one configuration/snapshot pair before any task starts.
    pub(crate) fn prepare(
        config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
    ) -> Result<(AuthorityServiceBootstrap, AuthorityRelayDrain), AuthorityServiceStartError> {
        let consensus = snapshot.consensus();
        let (runtime, max_parents) = AuthorityRuntime::new_with_relay_parent_limit(
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
            reorg_receiver,
            chunk_rx,
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
        let topology = AuthorityTaskTopology::start(
            handle,
            runtime.clone(),
            Arc::clone(&verification_cache),
            chunk_rx.clone(),
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
        let reorg_service = service.clone();
        let reorg_cancel = cancel.child_token();
        let reorg = handle.spawn(async move {
            run_ordered_reorg_driver(reorg_service, reorg_receiver, reorg_cancel).await
        });
        Ok(AuthorityServiceAssembly {
            service,
            generation: AuthorityGeneration {
                topology: Some(topology),
                reorg: Some(reorg),
                cancel,
                invalid: None,
            },
        })
    }

    pub(crate) async fn submit_remote(
        &self,
        tx: TransactionView,
        declared_cycles: u64,
        peer: PeerIndex,
    ) -> Result<AuthorityIngressDisposition, AuthorityServiceError> {
        loop {
            let signal = self.runtime.mutation_signal();
            let notified = signal.notified();
            match self
                .runtime
                .submit_remote_ingress(tx.clone(), declared_cycles, peer)
            {
                Ok(commit) => return Ok(map_ingress_commit(commit)),
                Err(RetainedIngressBoundaryError::Backpressure(
                    RetainedIngressBackpressure::EffectCapacity,
                )) => {
                    if !wait_or_cancel(&self.cancel, notified).await {
                        return Err(AuthorityServiceError::Cancelled);
                    }
                }
                Err(RetainedIngressBoundaryError::ResourceUnavailable) => {
                    return self
                        .publish_remote_pressure(tx, peer, RemoteIngressPressure::Allocation)
                        .await;
                }
                Err(RetainedIngressBoundaryError::Backpressure(pressure)) => {
                    let pressure = match pressure {
                        RetainedIngressBackpressure::TotalResources => {
                            RemoteIngressPressure::TotalResources
                        }
                        RetainedIngressBackpressure::RemoteResources => {
                            RemoteIngressPressure::RemoteResources
                        }
                        RetainedIngressBackpressure::PeerResources => {
                            RemoteIngressPressure::PeerResources
                        }
                        RetainedIngressBackpressure::ComputeResources => {
                            RemoteIngressPressure::ComputeResources
                        }
                        RetainedIngressBackpressure::ProposalCollision => {
                            RemoteIngressPressure::ProposalCollision
                        }
                        RetainedIngressBackpressure::EffectCapacity => continue,
                    };
                    return self.publish_remote_pressure(tx, peer, pressure).await;
                }
                Err(error) => return map_ingress_error(error),
            }
        }
    }

    async fn publish_remote_pressure(
        &self,
        tx: TransactionView,
        peer: PeerIndex,
        pressure: RemoteIngressPressure,
    ) -> Result<AuthorityIngressDisposition, AuthorityServiceError> {
        loop {
            let signal = self.runtime.mutation_signal();
            let notified = signal.notified();
            match self
                .runtime
                .reject_remote_ingress_pressure(tx.clone(), peer, pressure)
            {
                Ok(RetainedIngressCommit::Rejected) => {
                    return Ok(AuthorityIngressDisposition::Rejected);
                }
                Ok(
                    RetainedIngressCommit::Retained
                    | RetainedIngressCommit::AcceptedDuplicate
                    | RetainedIngressCommit::RemoteReleased
                    | RetainedIngressCommit::ProposalUnchanged,
                ) => {
                    return Err(AuthorityServiceError::Projection(
                        AuthorityProjectionFault::Effect,
                    ));
                }
                Err(RetainedIngressBoundaryError::Backpressure(
                    RetainedIngressBackpressure::EffectCapacity,
                )) => {
                    if !wait_or_cancel(&self.cancel, notified).await {
                        return Err(AuthorityServiceError::Cancelled);
                    }
                }
                Err(error) => return map_ingress_error(error),
            }
        }
    }

    pub(crate) async fn submit_proposal(
        &self,
        tx: TransactionView,
    ) -> Result<AuthorityIngressDisposition, AuthorityServiceError> {
        loop {
            let signal = self.runtime.mutation_signal();
            let notified = signal.notified();
            match self.runtime.submit_proposal_ingress(tx.clone()) {
                Ok(commit) => return Ok(map_ingress_commit(commit)),
                Err(RetainedIngressBoundaryError::Backpressure(
                    RetainedIngressBackpressure::EffectCapacity,
                )) => {
                    if !wait_or_cancel(&self.cancel, notified).await {
                        return Err(AuthorityServiceError::Cancelled);
                    }
                }
                Err(error) => return map_ingress_error(error),
            }
        }
    }

    pub(crate) async fn submit_local(
        &self,
        transaction: TransactionView,
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
        transaction: TransactionView,
    ) -> Result<Result<EntryCompleted, Reject>, AuthorityServiceError> {
        self.execute_direct(transaction, true).await
    }

    async fn execute_direct(
        &self,
        mut transaction: TransactionView,
        test_accept: bool,
    ) -> Result<Result<EntryCompleted, Reject>, AuthorityServiceError> {
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
                    return Err(AuthorityServiceError::Projection(
                        AuthorityProjectionFault::Membership,
                    ));
                }
            };
            let verified = match resolution {
                AuthorityDirectResolutionOutcome::Rejected(rejection) => {
                    let signal = self.runtime.mutation_signal();
                    let notified = signal.notified();
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
                            return Err(AuthorityServiceError::Projection(
                                AuthorityProjectionFault::Scheduler,
                            ));
                        }
                        Err(error) => match classify_direct_error(error) {
                            DirectErrorDisposition::Retry => continue,
                            DirectErrorDisposition::WaitEffect => {
                                if !wait_or_cancel(&self.cancel, notified).await {
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
                        .execute_direct_verification(request, Some(&mut command_rx))
                        .await
                    {
                        Ok(outcome) => outcome,
                        Err(DirectComputationError::StaleView) => continue,
                        Err(DirectComputationError::ResourceUnavailable) => {
                            return Ok(Err(direct_pressure_reject()));
                        }
                        Err(DirectComputationError::InvalidEvidence) => {
                            return Err(AuthorityServiceError::Projection(
                                AuthorityProjectionFault::Membership,
                            ));
                        }
                    }
                }
            };
            match verified {
                AuthorityDirectVerificationOutcome::Rejected(rejection) => {
                    let signal = self.runtime.mutation_signal();
                    let notified = signal.notified();
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
                            return Err(AuthorityServiceError::Projection(
                                AuthorityProjectionFault::Scheduler,
                            ));
                        }
                        Err(error) => match classify_direct_error(error) {
                            DirectErrorDisposition::Retry => continue,
                            DirectErrorDisposition::WaitEffect => {
                                if !wait_or_cancel(&self.cancel, notified).await {
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
                    let signal = self.runtime.mutation_signal();
                    let notified = signal.notified();
                    let outcome = match self.runtime.settle_verified_direct_admission(candidate) {
                        Ok(outcome) => outcome,
                        Err(error) => match classify_direct_error(error) {
                            DirectErrorDisposition::Retry => continue,
                            DirectErrorDisposition::WaitEffect => {
                                if !wait_or_cancel(&self.cancel, notified).await {
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
                                    transaction = retry.as_ref().clone();
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
                            return Err(AuthorityServiceError::Projection(
                                AuthorityProjectionFault::Scheduler,
                            ));
                        }
                    }
                }
            }
        }
    }

    async fn publish_cache_update(&self, update: VerificationCacheUpdate) {
        let (key, completed) = update.into_parts();
        self.verification_cache.write().await.put(key, completed);
    }

    pub(crate) async fn remove_local(&self, hash: &Byte32) -> Result<bool, AuthorityServiceError> {
        self.retry_administration(|| self.runtime.remove_local_transaction(hash))
            .await
    }

    pub(crate) async fn clear_pipeline(&self) -> Result<(), AuthorityServiceError> {
        self.retry_administration(|| self.runtime.clear_pipeline())
            .await
    }

    pub(crate) async fn clear_pool(
        &self,
        snapshot: Arc<Snapshot>,
    ) -> Result<(), AuthorityServiceError> {
        self.retry_administration(|| self.runtime.clear_pool(Arc::clone(&snapshot)))
            .await
    }

    async fn retry_administration<T>(
        &self,
        mut attempt: impl FnMut() -> Result<T, AuthorityAdministrationError>,
    ) -> Result<T, AuthorityServiceError> {
        loop {
            let signal = self.runtime.mutation_signal();
            let notified = signal.notified();
            match attempt() {
                Ok(value) => return Ok(value),
                Err(AuthorityAdministrationError::Allocation) => {
                    if !allocation_backoff_or_cancel(&self.cancel).await {
                        return Err(AuthorityServiceError::Cancelled);
                    }
                }
                Err(AuthorityAdministrationError::EffectCapacity) => {
                    if !wait_or_cancel(&self.cancel, notified).await {
                        return Err(AuthorityServiceError::Cancelled);
                    }
                }
                Err(error) => return Err(map_administration_error(error)),
            }
        }
    }

    pub(crate) async fn apply_chain_update(
        &self,
        arguments: ChainReorgArgs,
    ) -> Result<(), AuthorityServiceError> {
        let (detached, attached, proposals, snapshot) = arguments;
        let packaging = if self.block_assembler.is_some() {
            ChainPackaging::Package
        } else {
            ChainPackaging::ObserveOnly
        };
        let mut request =
            ChainUpdateRequest::new(detached, attached, proposals, snapshot, packaging);
        let mut command = loop {
            match request.prepare() {
                Ok(command) => break command,
                Err(failure) => {
                    let (error, returned) = failure.into_parts();
                    if error != ChainBoundaryError::Allocation {
                        return Err(map_chain_error(error));
                    }
                    request = returned;
                    if !allocation_backoff_or_cancel(&self.cancel).await {
                        return Err(AuthorityServiceError::Cancelled);
                    }
                }
            }
        };
        let committed = loop {
            match self.runtime.apply_chain_update(command) {
                Ok(committed) => break committed,
                Err(failure) => {
                    let (error, returned) = failure.into_parts();
                    if error != ChainBoundaryError::Allocation {
                        return Err(map_chain_error(error));
                    }
                    command = returned;
                    if !allocation_backoff_or_cancel(&self.cancel).await {
                        return Err(AuthorityServiceError::Cancelled);
                    }
                }
            }
        };
        // Fee estimation is a derived observer of the committed chain cut.
        // It must never run during preparation (a retried or rejected command
        // is not chain history), and candidate-uncle publication cannot veto
        // this independent post-commit projection.
        for block in &committed.attached_blocks {
            self.fee_estimator.commit_block(block);
        }
        if let Some(assembler) = &self.block_assembler {
            for uncle in committed.candidate_uncles {
                assembler
                    .receive_candidate_uncle(uncle)
                    .map_err(|_| AuthorityServiceError::CounterExhausted)?;
            }
        }
        drop(committed.snapshot);
        Ok(())
    }

    pub(crate) fn receive_candidate_uncle(
        &self,
        uncle: UncleBlockView,
    ) -> Result<(), AuthorityServiceError> {
        if let Some(assembler) = &self.block_assembler {
            assembler
                .receive_candidate_uncle(uncle)
                .map_err(|_| AuthorityServiceError::CounterExhausted)?;
        }
        Ok(())
    }

    pub(crate) async fn block_template(
        &self,
    ) -> Result<ckb_jsonrpc_types::BlockTemplate, AuthorityServiceError> {
        match &self.block_assembler {
            Some(assembler) => Ok(assembler.current_template().await),
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

    pub(crate) fn pool_summary(&self) -> Result<AuthorityPoolSummary, AuthorityServiceError> {
        self.runtime.pool_summary().map_err(map_query_error)
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
        proposals: Vec<ProposalShortId>,
    ) -> Result<HashMap<ProposalShortId, (TransactionView, u64)>, AuthorityServiceError> {
        self.runtime
            .accepted_with_cycles(proposals)
            .map_err(map_query_error)
    }

    pub(crate) fn pool_ids(
        &self,
    ) -> Result<ckb_types::core::tx_pool::TxPoolIds, AuthorityServiceError> {
        self.runtime.pool_ids().map_err(map_query_error)
    }

    pub(crate) fn all_entry_info(
        &self,
    ) -> Result<ckb_types::core::tx_pool::TxPoolEntryInfo, AuthorityServiceError> {
        self.runtime.all_entry_info().map_err(map_query_error)
    }

    pub(crate) fn pool_detail(
        &self,
        hash: &Byte32,
    ) -> Result<Option<ckb_types::core::tx_pool::PoolTxDetailInfo>, AuthorityServiceError> {
        self.runtime.pool_detail(hash).map_err(map_query_error)
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
        let snapshot = crate::persisted::PersistenceSnapshot {
            accepted: accepted.into_iter().map(Arc::unwrap_or_clone).collect(),
            recovery: recovery.into_iter().map(Arc::unwrap_or_clone).collect(),
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

    pub(crate) fn fee_estimate_receipt(
        &self,
    ) -> Result<FeeEstimateReadReceipt, AuthorityServiceError> {
        self.runtime.fee_estimate_receipt().map_err(map_query_error)
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
        if let Some(record) = self
            .pending_recent_reject(hash)
            .map_err(AuthorityDerivedError::Authority)?
        {
            return Ok(Some(record));
        }
        match &self.recent_reject {
            Some(recent) => recent.get(hash).map_err(AuthorityDerivedError::External),
            None => Ok(None),
        }
    }

    pub(crate) fn estimate_fee_rate(
        &self,
        estimate_mode: EstimateMode,
        enable_fallback: bool,
    ) -> Result<FeeRate, AuthorityDerivedError> {
        let entries = self
            .all_entry_info()
            .map_err(AuthorityDerivedError::Authority)?;
        match self.fee_estimator.estimate_fee_rate(estimate_mode, entries) {
            Ok(rate) => Ok(rate),
            Err(error) if !enable_fallback => Err(AuthorityDerivedError::External(error.into())),
            Err(_) => {
                let target = FeeEstimator::target_blocks_for_estimate_mode(estimate_mode);
                self.fee_estimate_receipt()
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
    ) -> Result<Option<String>, AuthorityServiceError> {
        self.runtime
            .pending_recent_reject(hash)
            .map_err(|_| AuthorityServiceError::Projection(AuthorityProjectionFault::Effect))
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
        AuthorityInternalPlugError::Capacity
        | AuthorityInternalPlugError::ResourceUnavailable
        | AuthorityInternalPlugError::ProposalCollision => direct_pressure_reject(),
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

fn map_topology_start_error(error: AuthorityTopologyStartError) -> AuthorityServiceStartError {
    match error {
        AuthorityTopologyStartError::Cancelled => AuthorityServiceStartError::Cancelled,
        AuthorityTopologyStartError::EffectPublisherClaimed => {
            AuthorityServiceStartError::EffectPublisherClaimed
        }
        AuthorityTopologyStartError::Worker(_) => AuthorityServiceStartError::WorkerAllocation,
    }
}

fn map_ingress_commit(commit: RetainedIngressCommit) -> AuthorityIngressDisposition {
    match commit {
        RetainedIngressCommit::Retained => AuthorityIngressDisposition::Retained,
        RetainedIngressCommit::AcceptedDuplicate => AuthorityIngressDisposition::AcceptedDuplicate,
        RetainedIngressCommit::RemoteReleased => AuthorityIngressDisposition::RemoteReleased,
        RetainedIngressCommit::ProposalUnchanged => AuthorityIngressDisposition::ProposalUnchanged,
        RetainedIngressCommit::Rejected => AuthorityIngressDisposition::Rejected,
    }
}

fn map_ingress_error(
    error: RetainedIngressBoundaryError,
) -> Result<AuthorityIngressDisposition, AuthorityServiceError> {
    match error {
        RetainedIngressBoundaryError::InvalidEvidence => Err(AuthorityServiceError::Projection(
            AuthorityProjectionFault::Membership,
        )),
        RetainedIngressBoundaryError::ResourceUnavailable => Ok(
            AuthorityIngressDisposition::Pressure(AuthorityIngressPressure::Allocation),
        ),
        RetainedIngressBoundaryError::Backpressure(pressure) => {
            Ok(AuthorityIngressDisposition::Pressure(match pressure {
                RetainedIngressBackpressure::TotalResources => {
                    AuthorityIngressPressure::TotalResources
                }
                RetainedIngressBackpressure::RemoteResources => {
                    AuthorityIngressPressure::RemoteResources
                }
                RetainedIngressBackpressure::PeerResources => {
                    AuthorityIngressPressure::PeerResources
                }
                RetainedIngressBackpressure::ComputeResources => {
                    AuthorityIngressPressure::ComputeResources
                }
                RetainedIngressBackpressure::EffectCapacity => {
                    AuthorityIngressPressure::EffectCapacity
                }
                RetainedIngressBackpressure::ProposalCollision => {
                    AuthorityIngressPressure::ProposalCollision
                }
            }))
        }
        RetainedIngressBoundaryError::PeerRevoked(_) => {
            Ok(AuthorityIngressDisposition::PeerRevoked)
        }
        RetainedIngressBoundaryError::LifecycleClosed => {
            Err(AuthorityServiceError::LifecycleClosed)
        }
        RetainedIngressBoundaryError::Fault(fault) => {
            Err(AuthorityServiceError::Projection(fault.into()))
        }
    }
}

enum DirectErrorDisposition {
    Retry,
    WaitEffect,
    Reject(Reject),
    Service(AuthorityServiceError),
}

fn classify_direct_error(error: AuthorityDirectAdmissionError) -> DirectErrorDisposition {
    match error {
        AuthorityDirectAdmissionError::Stale => DirectErrorDisposition::Retry,
        AuthorityDirectAdmissionError::ResourceUnavailable
        | AuthorityDirectAdmissionError::ProposalCollision => {
            DirectErrorDisposition::Reject(direct_pressure_reject())
        }
        AuthorityDirectAdmissionError::EffectCapacity => DirectErrorDisposition::WaitEffect,
        AuthorityDirectAdmissionError::LifecycleClosed => {
            DirectErrorDisposition::Service(AuthorityServiceError::LifecycleClosed)
        }
        AuthorityDirectAdmissionError::Fault(fault) => {
            DirectErrorDisposition::Service(AuthorityServiceError::Projection(fault.into()))
        }
    }
}

fn direct_pressure_reject() -> Reject {
    Reject::Full("tx-pool cannot admit the transaction under current resource limits".to_owned())
}

async fn wait_or_cancel(
    cancel: &CancellationToken,
    notified: tokio::sync::futures::Notified<'_>,
) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = notified => true,
    }
}

async fn allocation_backoff_or_cancel(cancel: &CancellationToken) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(Duration::from_millis(1)) => true,
    }
}

async fn run_ordered_reorg_driver(
    service: AuthorityService,
    mut receiver: mpsc::Receiver<crate::service::Notify<ChainReorgArgs>>,
    cancel: CancellationToken,
) -> Result<(), AuthorityGenerationInvalidity> {
    loop {
        let update = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            update = receiver.recv() => update,
        };
        let Some(crate::service::Notify { arguments }) = update else {
            return Ok(());
        };
        match service.apply_chain_update(arguments).await {
            Ok(()) => {}
            Err(AuthorityServiceError::Cancelled) if cancel.is_cancelled() => return Ok(()),
            Err(error) => {
                AuthorityService::settle_operation_error(error)?;
                return Ok(());
            }
        }
    }
}

fn map_administration_error(error: AuthorityAdministrationError) -> AuthorityServiceError {
    match error {
        AuthorityAdministrationError::Allocation => AuthorityServiceError::ResourceUnavailable,
        AuthorityAdministrationError::EffectCapacity => AuthorityServiceError::EffectCapacity,
        AuthorityAdministrationError::LifecycleClosed => AuthorityServiceError::LifecycleClosed,
        AuthorityAdministrationError::Fault(fault) => {
            AuthorityServiceError::Projection(fault.into())
        }
    }
}

fn map_chain_error(error: ChainBoundaryError) -> AuthorityServiceError {
    match error {
        ChainBoundaryError::Allocation => AuthorityServiceError::ResourceUnavailable,
        ChainBoundaryError::LifecycleClosed => AuthorityServiceError::LifecycleClosed,
        ChainBoundaryError::CounterExhausted => AuthorityServiceError::CounterExhausted,
        ChainBoundaryError::InvalidFacts | ChainBoundaryError::InvalidSnapshotEvidence => {
            AuthorityServiceError::InvalidChainEvidence
        }
        ChainBoundaryError::Fault(fault) => AuthorityServiceError::Projection(fault.into()),
    }
}

fn map_query_error(error: AuthorityQueryError) -> AuthorityServiceError {
    match error {
        AuthorityQueryError::Allocation => AuthorityServiceError::ResourceUnavailable,
        AuthorityQueryError::Arithmetic => {
            AuthorityServiceError::Projection(AuthorityProjectionFault::Resource)
        }
        AuthorityQueryError::Projection
        | AuthorityQueryError::AcceptedCycle
        | AuthorityQueryError::RecoveryCycle => {
            AuthorityServiceError::Projection(AuthorityProjectionFault::Membership)
        }
    }
}

fn map_template_read_error(error: TemplateReadError) -> AuthorityServiceError {
    match error {
        TemplateReadError::Allocation => AuthorityServiceError::ResourceUnavailable,
        TemplateReadError::Arithmetic => {
            AuthorityServiceError::Projection(AuthorityProjectionFault::Resource)
        }
        TemplateReadError::Projection | TemplateReadError::CausalCycle => {
            AuthorityServiceError::Projection(AuthorityProjectionFault::Membership)
        }
    }
}
