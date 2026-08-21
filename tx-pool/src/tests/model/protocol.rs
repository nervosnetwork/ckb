use super::{
    kernel::{KernelCommand, KernelDisposition, KernelStep},
    state::{ModelInvariantError, ModelLimits, Omega, RulesId, ViewId},
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RequestId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RequestKind {
    Ordinary { response: bool },
    Notification,
    OrderedChain { response: bool },
}

impl RequestKind {
    fn has_response(self) -> bool {
        match self {
            Self::Ordinary { response } | Self::OrderedChain { response } => response,
            Self::Notification => false,
        }
    }

    fn is_ordered(self) -> bool {
        matches!(self, Self::OrderedChain { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PayloadLocation {
    Queued,
    HandlerOwned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResponseEndpoint {
    Attached,
    Abandoned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResponseResult {
    NotApplicable,
    Sent,
    Dropped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PayloadCost {
    pub(super) items: u16,
    pub(super) bytes: u32,
}

impl PayloadCost {
    pub(super) const fn small() -> Self {
        Self { items: 1, bytes: 4 }
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            items: self.items.checked_add(other.items)?,
            bytes: self.bytes.checked_add(other.bytes)?,
        })
    }

    fn fits(self, limit: Self) -> bool {
        self.items <= limit.items && self.bytes <= limit.bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RequestRecord {
    pub(super) kind: RequestKind,
    pub(super) payload: PayloadLocation,
    pub(super) response: Option<ResponseEndpoint>,
    pub(super) cost: PayloadCost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProtocolLimits {
    pub(super) ordinary_queue: usize,
    pub(super) ordered_queue: usize,
    pub(super) handlers: usize,
    pub(super) ordered_handlers: usize,
    pub(super) ordinary_queue_cost: PayloadCost,
    pub(super) ordered_queue_cost: PayloadCost,
    pub(super) handler_cost: PayloadCost,
    pub(super) ordered_handler_cost: PayloadCost,
}

impl ProtocolLimits {
    pub(super) const fn small() -> Self {
        Self {
            ordinary_queue: 2,
            ordered_queue: 1,
            handlers: 2,
            ordered_handlers: 1,
            ordinary_queue_cost: PayloadCost {
                items: 4,
                bytes: 32,
            },
            ordered_queue_cost: PayloadCost {
                items: 4,
                bytes: 32,
            },
            handler_cost: PayloadCost {
                items: 4,
                bytes: 32,
            },
            ordered_handler_cost: PayloadCost {
                items: 4,
                bytes: 32,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ControllerProtocol {
    pub(super) requests: BTreeMap<RequestId, RequestRecord>,
    pub(super) limits: ProtocolLimits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DerivedComponent {
    Disabled,
    Enabled { source: u16, published: u16 },
    Degraded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DerivedHealth {
    Disabled,
    Enabled,
    Degraded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DerivedState {
    pub(super) template: DerivedComponent,
    pub(super) recent_reject: DerivedHealth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lifecycle {
    Constructing,
    Initializing,
    Running,
    Draining,
    Stopped,
    StartupFailed,
}

/// Semantic task classes for one service generation. Dynamic handler and
/// worker multiplicities refine these classes; they do not create new owners.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum GenerationTaskRole {
    DispatcherRoot,
    MessageHandler,
    ChainControl,
    ComputeCoordinator,
    ComputeWorker,
    Ready,
    Maintenance,
    EffectPublisher,
    VerificationCache,
    TemplateLane,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GenerationTaskOwner {
    ProcessRuntimeGuard,
    DispatcherJoinSet,
    AuthorityGeneration,
    AuthorityTaskTopology,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GenerationTaskCriticality {
    LifecycleRoot,
    AuthorityCapability,
    DerivedProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GenerationTaskContract {
    pub(super) role: GenerationTaskRole,
    pub(super) owner: GenerationTaskOwner,
    pub(super) criticality: GenerationTaskCriticality,
    pub(super) join_cut: Option<ShutdownPhase>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GenerationTaskExit {
    LifecycleCancellation,
    OwnerClosed,
    OrdinaryEndpointFailure,
    StructuralFailure,
    JoinFailure,
    Timeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GenerationTaskDisposition {
    Continue,
    Join,
    RequestShutdown,
    DerivedDegraded,
    PersistenceForbidden,
}

pub(super) const GENERATION_TASK_ROLES: [GenerationTaskRole; 10] = [
    GenerationTaskRole::DispatcherRoot,
    GenerationTaskRole::MessageHandler,
    GenerationTaskRole::ChainControl,
    GenerationTaskRole::ComputeCoordinator,
    GenerationTaskRole::ComputeWorker,
    GenerationTaskRole::Ready,
    GenerationTaskRole::Maintenance,
    GenerationTaskRole::EffectPublisher,
    GenerationTaskRole::VerificationCache,
    GenerationTaskRole::TemplateLane,
];

pub(super) const fn generation_task_contract(role: GenerationTaskRole) -> GenerationTaskContract {
    use GenerationTaskCriticality::{AuthorityCapability, DerivedProjection, LifecycleRoot};
    use GenerationTaskOwner::{
        AuthorityGeneration, AuthorityTaskTopology, DispatcherJoinSet, ProcessRuntimeGuard,
    };
    match role {
        GenerationTaskRole::DispatcherRoot => GenerationTaskContract {
            role,
            owner: ProcessRuntimeGuard,
            criticality: LifecycleRoot,
            join_cut: None,
        },
        GenerationTaskRole::MessageHandler => GenerationTaskContract {
            role,
            owner: DispatcherJoinSet,
            criticality: AuthorityCapability,
            join_cut: Some(ShutdownPhase::HandlersDrained),
        },
        GenerationTaskRole::ChainControl => GenerationTaskContract {
            role,
            owner: AuthorityGeneration,
            criticality: AuthorityCapability,
            join_cut: Some(ShutdownPhase::ChainControlJoined),
        },
        GenerationTaskRole::ComputeCoordinator
        | GenerationTaskRole::ComputeWorker
        | GenerationTaskRole::Ready
        | GenerationTaskRole::Maintenance => GenerationTaskContract {
            role,
            owner: AuthorityTaskTopology,
            criticality: AuthorityCapability,
            join_cut: Some(ShutdownPhase::AuthorityWorkersJoined),
        },
        GenerationTaskRole::EffectPublisher => GenerationTaskContract {
            role,
            owner: AuthorityTaskTopology,
            criticality: AuthorityCapability,
            join_cut: Some(ShutdownPhase::EffectsDrained),
        },
        GenerationTaskRole::VerificationCache | GenerationTaskRole::TemplateLane => {
            GenerationTaskContract {
                role,
                owner: AuthorityTaskTopology,
                criticality: DerivedProjection,
                join_cut: Some(ShutdownPhase::DerivedTasksJoined),
            }
        }
    }
}

/// Total task-exit algebra. External endpoint failure is consumed inside the
/// publisher or notification lane and therefore cannot become a task exit or
/// a service-stop request. Structural task loss is classified only by the
/// capability domain it owns.
pub(super) const fn generation_task_disposition(
    role: GenerationTaskRole,
    exit: GenerationTaskExit,
) -> GenerationTaskDisposition {
    match exit {
        GenerationTaskExit::LifecycleCancellation => GenerationTaskDisposition::Join,
        GenerationTaskExit::OrdinaryEndpointFailure => GenerationTaskDisposition::Continue,
        GenerationTaskExit::StructuralFailure
        | GenerationTaskExit::JoinFailure
        | GenerationTaskExit::Timeout => match generation_task_contract(role).criticality {
            GenerationTaskCriticality::DerivedProjection => {
                GenerationTaskDisposition::DerivedDegraded
            }
            GenerationTaskCriticality::LifecycleRoot
            | GenerationTaskCriticality::AuthorityCapability => {
                GenerationTaskDisposition::PersistenceForbidden
            }
        },
        GenerationTaskExit::OwnerClosed => match role {
            GenerationTaskRole::DispatcherRoot
            | GenerationTaskRole::ChainControl
            | GenerationTaskRole::ComputeCoordinator
            | GenerationTaskRole::EffectPublisher => GenerationTaskDisposition::RequestShutdown,
            GenerationTaskRole::VerificationCache | GenerationTaskRole::TemplateLane => {
                GenerationTaskDisposition::DerivedDegraded
            }
            GenerationTaskRole::MessageHandler
            | GenerationTaskRole::ComputeWorker
            | GenerationTaskRole::Ready
            | GenerationTaskRole::Maintenance => GenerationTaskDisposition::Continue,
        },
    }
}

/// Readiness of the completion input while one compute completion is already
/// blocked behind the committed-effect frontier during shutdown.
///
/// An open empty receiver is pending. A disconnected receiver is permanently
/// ready and returns `None`; those states therefore cannot be quotiented even
/// though neither carries another completion value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelCoordinatorCompletionReadiness {
    OpenPending,
    DisconnectedReady,
    Closed,
}

/// Exact wait cut for a nonempty set of finished compute capabilities blocked
/// behind committed-effect publication during coordinator shutdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelEffectBlockedShutdownCut {
    completion: ModelCoordinatorCompletionReadiness,
    effect_waiters: usize,
    effect_notification_ready: bool,
}

impl ModelEffectBlockedShutdownCut {
    pub(super) const fn new(
        completion: ModelCoordinatorCompletionReadiness,
        effect_waiters: usize,
        effect_notification_ready: bool,
    ) -> Self {
        Self {
            completion,
            effect_waiters,
            effect_notification_ready,
        }
    }

    pub(super) const fn completion(self) -> ModelCoordinatorCompletionReadiness {
        self.completion
    }

    pub(super) const fn effect_waiters(self) -> usize {
        self.effect_waiters
    }

    pub(super) const fn with_effect_notification(self) -> Self {
        Self {
            effect_notification_ready: true,
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelCoordinatorShutdownObservation {
    Pending,
    CompletionIngressClosed,
    EffectWaitersPromoted,
}

/// One corrected shutdown step. A disconnected receive has exactly one
/// observation: it terminalizes the completion ingress. The absorbing Closed
/// state can never compete in the next wait, so every later effect-capacity
/// notification strictly removes one waiter from the finite drain rank.
pub(super) const fn completion_ingress_shutdown_step(
    cut: ModelEffectBlockedShutdownCut,
) -> (
    ModelEffectBlockedShutdownCut,
    ModelCoordinatorShutdownObservation,
) {
    match cut.completion {
        ModelCoordinatorCompletionReadiness::DisconnectedReady => (
            ModelEffectBlockedShutdownCut {
                completion: ModelCoordinatorCompletionReadiness::Closed,
                ..cut
            },
            ModelCoordinatorShutdownObservation::CompletionIngressClosed,
        ),
        ModelCoordinatorCompletionReadiness::OpenPending
        | ModelCoordinatorCompletionReadiness::Closed
            if cut.effect_notification_ready && cut.effect_waiters > 0 =>
        {
            (
                ModelEffectBlockedShutdownCut {
                    effect_waiters: cut.effect_waiters - 1,
                    effect_notification_ready: false,
                    ..cut
                },
                ModelCoordinatorShutdownObservation::EffectWaitersPromoted,
            )
        }
        ModelCoordinatorCompletionReadiness::OpenPending
        | ModelCoordinatorCompletionReadiness::Closed => {
            (cut, ModelCoordinatorShutdownObservation::Pending)
        }
    }
}

/// Ordered shutdown sub-protocol for the persistence eligibility cut.
///
/// Cancellation may wake several tasks concurrently, but persistence is
/// legal only after this ownership order has completed. Derived degradation
/// is recorded without retroactively invalidating the authority cut; loss of
/// an authority-owning capability permanently forbids persistence. A later
/// external write failure is terminal but does not rewrite authority history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShutdownPhase {
    Running,
    IngressClosed,
    HandlersDrained,
    ChainControlJoined,
    AuthorityWorkersJoined,
    EffectsClosed,
    EffectsDrained,
    DerivedTasksJoined,
    PersistenceCaptured,
    Persisted,
    PersistenceFailed,
    Invalidating,
    AbortRequested,
    InvalidTasksJoined,
    PersistenceForbidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShutdownAction {
    CloseIngress,
    DrainHandlers,
    JoinChainControl,
    JoinAuthorityWorkers,
    CloseEffects,
    DrainEffects,
    JoinDerivedTasks,
    CapturePersistence,
    WritePersistence,
    PersistenceWriteFailed,
    DerivedTaskFailed,
    AuthorityCapabilityLost,
    RequestAbort,
    JoinAbortedTasks,
    ReportPersistenceForbidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShutdownDisposition {
    Advanced(ShutdownPhase),
    DerivedDegraded(ShutdownPhase),
    PersistenceFailed,
    PersistenceForbidden,
    OutOfOrder(ShutdownPhase),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ShutdownProtocol {
    phase: ShutdownPhase,
    derived_degraded: bool,
}

impl ShutdownProtocol {
    pub(super) const fn running() -> Self {
        Self {
            phase: ShutdownPhase::Running,
            derived_degraded: false,
        }
    }

    pub(super) const fn phase(self) -> ShutdownPhase {
        self.phase
    }

    pub(super) const fn derived_degraded(self) -> bool {
        self.derived_degraded
    }

    pub(super) fn step(&mut self, action: ShutdownAction) -> ShutdownDisposition {
        if action == ShutdownAction::AuthorityCapabilityLost {
            if matches!(
                self.phase,
                ShutdownPhase::Persisted
                    | ShutdownPhase::PersistenceFailed
                    | ShutdownPhase::Invalidating
                    | ShutdownPhase::AbortRequested
                    | ShutdownPhase::InvalidTasksJoined
            ) {
                return ShutdownDisposition::OutOfOrder(self.phase);
            }
            self.phase = ShutdownPhase::Invalidating;
            return ShutdownDisposition::Advanced(ShutdownPhase::Invalidating);
        }
        if self.phase == ShutdownPhase::PersistenceForbidden {
            return ShutdownDisposition::PersistenceForbidden;
        }
        if action == ShutdownAction::DerivedTaskFailed {
            if matches!(
                self.phase,
                ShutdownPhase::DerivedTasksJoined
                    | ShutdownPhase::PersistenceCaptured
                    | ShutdownPhase::Persisted
                    | ShutdownPhase::PersistenceFailed
            ) {
                return ShutdownDisposition::OutOfOrder(self.phase);
            }
            self.derived_degraded = true;
            return ShutdownDisposition::DerivedDegraded(self.phase);
        }
        let next = match (self.phase, action) {
            (ShutdownPhase::Running, ShutdownAction::CloseIngress) => ShutdownPhase::IngressClosed,
            (ShutdownPhase::IngressClosed, ShutdownAction::DrainHandlers) => {
                ShutdownPhase::HandlersDrained
            }
            (ShutdownPhase::HandlersDrained, ShutdownAction::JoinChainControl) => {
                ShutdownPhase::ChainControlJoined
            }
            (ShutdownPhase::ChainControlJoined, ShutdownAction::JoinAuthorityWorkers) => {
                ShutdownPhase::AuthorityWorkersJoined
            }
            (ShutdownPhase::AuthorityWorkersJoined, ShutdownAction::CloseEffects) => {
                ShutdownPhase::EffectsClosed
            }
            (ShutdownPhase::EffectsClosed, ShutdownAction::DrainEffects) => {
                ShutdownPhase::EffectsDrained
            }
            (ShutdownPhase::EffectsDrained, ShutdownAction::JoinDerivedTasks) => {
                ShutdownPhase::DerivedTasksJoined
            }
            (ShutdownPhase::DerivedTasksJoined, ShutdownAction::CapturePersistence) => {
                ShutdownPhase::PersistenceCaptured
            }
            (ShutdownPhase::PersistenceCaptured, ShutdownAction::WritePersistence) => {
                ShutdownPhase::Persisted
            }
            (ShutdownPhase::PersistenceCaptured, ShutdownAction::PersistenceWriteFailed) => {
                self.phase = ShutdownPhase::PersistenceFailed;
                return ShutdownDisposition::PersistenceFailed;
            }
            (ShutdownPhase::Invalidating, ShutdownAction::RequestAbort) => {
                ShutdownPhase::AbortRequested
            }
            (ShutdownPhase::AbortRequested, ShutdownAction::JoinAbortedTasks) => {
                ShutdownPhase::InvalidTasksJoined
            }
            (ShutdownPhase::InvalidTasksJoined, ShutdownAction::ReportPersistenceForbidden) => {
                self.phase = ShutdownPhase::PersistenceForbidden;
                return ShutdownDisposition::PersistenceForbidden;
            }
            _ => return ShutdownDisposition::OutOfOrder(self.phase),
        };
        self.phase = next;
        ShutdownDisposition::Advanced(next)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SystemState {
    pub(super) authority: Option<Omega>,
    pub(super) protocol: ControllerProtocol,
    pub(super) derived: DerivedState,
    pub(super) lifecycle: Lifecycle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KernelAccess {
    Ordinary,
    Initialization,
    Drain,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SystemEvent {
    Enqueue {
        request: RequestId,
        kind: RequestKind,
        cost: PayloadCost,
    },
    Dispatch(RequestId),
    Finish {
        request: RequestId,
        send_response: bool,
    },
    AbandonReceiver(RequestId),
    Assemble {
        limits: ModelLimits,
        view: ViewId,
        rules: RulesId,
        succeed: bool,
    },
    Ready,
    InitializationReplayFailed,
    BeginDrain,
    FinishDrain,
    Kernel {
        access: KernelAccess,
        command: KernelCommand,
    },
    PublishTemplate {
        captured_source: u16,
    },
    DegradeRecentReject,
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Keeping the step inline makes allocation part of the modeled transition
// explicit; boxing it would add an allocation that the production quotient
// does not own.
#[allow(clippy::large_enum_variant)]
pub(super) enum SystemDisposition {
    Enqueued(RequestId),
    QueueFull(RequestId),
    ChannelClosed(RequestId),
    DuplicateRequest(RequestId),
    Dispatched(RequestId),
    RequestUnavailable(RequestId),
    Finished {
        request: RequestId,
        response: ResponseResult,
    },
    ReceiverAbandoned(RequestId),
    Assembled,
    StartupFailed,
    InitializationDraining,
    Running,
    Draining,
    DrainPending,
    Stopped,
    KernelUnavailable,
    Kernel(KernelStep),
    TemplatePublished(u16),
    StaleTemplate(u16),
    DerivedDegraded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SystemInvariantError {
    AuthorityLifecycle,
    QueueBound,
    PayloadBound,
    PayloadOverflow,
    HandlerBound,
    ResponseShape,
    DerivedPublicationOrder,
    Kernel(ModelInvariantError),
}

impl SystemState {
    pub(super) fn constructing(limits: ProtocolLimits) -> Self {
        Self {
            authority: None,
            protocol: ControllerProtocol {
                requests: BTreeMap::new(),
                limits,
            },
            derived: DerivedState {
                template: DerivedComponent::Disabled,
                recent_reject: DerivedHealth::Disabled,
            },
            lifecycle: Lifecycle::Constructing,
        }
    }

    pub(super) fn step(&mut self, event: SystemEvent) -> SystemDisposition {
        match event {
            SystemEvent::Enqueue {
                request,
                kind,
                cost,
            } => self.enqueue(request, kind, cost),
            SystemEvent::Dispatch(request) => self.dispatch(request),
            SystemEvent::Finish {
                request,
                send_response,
            } => self.finish(request, send_response),
            SystemEvent::AbandonReceiver(request) => self.abandon_receiver(request),
            SystemEvent::Assemble {
                limits,
                view,
                rules,
                succeed,
            } => self.assemble(limits, view, rules, succeed),
            SystemEvent::Ready => self.ready(),
            SystemEvent::InitializationReplayFailed => self.initialization_replay_failed(),
            SystemEvent::BeginDrain => self.begin_drain(),
            SystemEvent::FinishDrain => self.finish_drain(),
            SystemEvent::Kernel { access, command } => self.kernel(access, command),
            SystemEvent::PublishTemplate { captured_source } => {
                self.publish_template(captured_source)
            }
            SystemEvent::DegradeRecentReject => {
                self.derived.recent_reject = DerivedHealth::Degraded;
                SystemDisposition::DerivedDegraded
            }
        }
    }

    pub(super) fn check_invariants(&self) -> Result<(), SystemInvariantError> {
        let should_have_authority = matches!(
            self.lifecycle,
            Lifecycle::Initializing | Lifecycle::Running | Lifecycle::Draining
        );
        if should_have_authority != self.authority.is_some() {
            return Err(SystemInvariantError::AuthorityLifecycle);
        }
        if let Some(authority) = &self.authority {
            authority
                .check_invariants()
                .map_err(SystemInvariantError::Kernel)?;
        }
        if matches!(
            self.derived.template,
            DerivedComponent::Enabled { source, published } if published > source
        ) {
            return Err(SystemInvariantError::DerivedPublicationOrder);
        }

        let mut ordinary_queued = 0usize;
        let mut ordered_queued = 0usize;
        let mut handlers = 0usize;
        let mut ordered_handlers = 0usize;
        let mut ordinary_queued_cost = PayloadCost { items: 0, bytes: 0 };
        let mut ordered_queued_cost = PayloadCost { items: 0, bytes: 0 };
        let mut handler_cost = PayloadCost { items: 0, bytes: 0 };
        let mut ordered_handler_cost = PayloadCost { items: 0, bytes: 0 };
        for request in self.protocol.requests.values() {
            match request.payload {
                PayloadLocation::Queued if request.kind.is_ordered() => {
                    ordered_queued = ordered_queued.saturating_add(1);
                    ordered_queued_cost = ordered_queued_cost
                        .checked_add(request.cost)
                        .ok_or(SystemInvariantError::PayloadOverflow)?;
                }
                PayloadLocation::Queued => {
                    ordinary_queued = ordinary_queued.saturating_add(1);
                    ordinary_queued_cost = ordinary_queued_cost
                        .checked_add(request.cost)
                        .ok_or(SystemInvariantError::PayloadOverflow)?;
                }
                PayloadLocation::HandlerOwned => {
                    if request.kind.is_ordered() {
                        ordered_handlers = ordered_handlers.saturating_add(1);
                        ordered_handler_cost = ordered_handler_cost
                            .checked_add(request.cost)
                            .ok_or(SystemInvariantError::PayloadOverflow)?;
                    } else {
                        handlers = handlers.saturating_add(1);
                        handler_cost = handler_cost
                            .checked_add(request.cost)
                            .ok_or(SystemInvariantError::PayloadOverflow)?;
                    }
                }
            }
            if request.kind.has_response() != request.response.is_some() {
                return Err(SystemInvariantError::ResponseShape);
            }
        }
        if ordinary_queued > self.protocol.limits.ordinary_queue
            || ordered_queued > self.protocol.limits.ordered_queue
        {
            return Err(SystemInvariantError::QueueBound);
        }
        if handlers > self.protocol.limits.handlers {
            return Err(SystemInvariantError::HandlerBound);
        }
        if ordered_handlers > self.protocol.limits.ordered_handlers {
            return Err(SystemInvariantError::HandlerBound);
        }
        if !ordinary_queued_cost.fits(self.protocol.limits.ordinary_queue_cost)
            || !ordered_queued_cost.fits(self.protocol.limits.ordered_queue_cost)
            || !handler_cost.fits(self.protocol.limits.handler_cost)
            || !ordered_handler_cost.fits(self.protocol.limits.ordered_handler_cost)
        {
            return Err(SystemInvariantError::PayloadBound);
        }
        Ok(())
    }

    fn enqueue(
        &mut self,
        request: RequestId,
        kind: RequestKind,
        cost: PayloadCost,
    ) -> SystemDisposition {
        if self.protocol.requests.contains_key(&request) {
            return SystemDisposition::DuplicateRequest(request);
        }
        if matches!(
            self.lifecycle,
            Lifecycle::Draining | Lifecycle::Stopped | Lifecycle::StartupFailed
        ) {
            return SystemDisposition::ChannelClosed(request);
        }
        let queued = self
            .protocol
            .requests
            .values()
            .filter(|record| {
                record.payload == PayloadLocation::Queued
                    && record.kind.is_ordered() == kind.is_ordered()
            })
            .count();
        let bound = if kind.is_ordered() {
            self.protocol.limits.ordered_queue
        } else {
            self.protocol.limits.ordinary_queue
        };
        if queued >= bound {
            return SystemDisposition::QueueFull(request);
        }
        let queued_cost = self
            .protocol
            .requests
            .values()
            .filter(|record| {
                record.payload == PayloadLocation::Queued
                    && record.kind.is_ordered() == kind.is_ordered()
            })
            .try_fold(PayloadCost { items: 0, bytes: 0 }, |total, record| {
                total.checked_add(record.cost)
            });
        let cost_bound = if kind.is_ordered() {
            self.protocol.limits.ordered_queue_cost
        } else {
            self.protocol.limits.ordinary_queue_cost
        };
        if queued_cost
            .and_then(|total| total.checked_add(cost))
            .is_none_or(|total| !total.fits(cost_bound))
        {
            return SystemDisposition::QueueFull(request);
        }
        let has_response = kind.has_response();
        self.protocol.requests.insert(
            request,
            RequestRecord {
                kind,
                payload: PayloadLocation::Queued,
                response: has_response.then_some(ResponseEndpoint::Attached),
                cost,
            },
        );
        SystemDisposition::Enqueued(request)
    }

    fn dispatch(&mut self, request: RequestId) -> SystemDisposition {
        let Some(record) = self.protocol.requests.get(&request) else {
            return SystemDisposition::RequestUnavailable(request);
        };
        let ordered = record.kind.is_ordered();
        if ordered {
            if !matches!(self.lifecycle, Lifecycle::Initializing | Lifecycle::Running) {
                return SystemDisposition::RequestUnavailable(request);
            }
        } else if self.lifecycle != Lifecycle::Running {
            return SystemDisposition::RequestUnavailable(request);
        }
        let handlers = self
            .protocol
            .requests
            .values()
            .filter(|record| {
                record.payload == PayloadLocation::HandlerOwned
                    && record.kind.is_ordered() == ordered
            })
            .count();
        let handler_bound = if ordered {
            self.protocol.limits.ordered_handlers
        } else {
            self.protocol.limits.handlers
        };
        if handlers >= handler_bound {
            return SystemDisposition::RequestUnavailable(request);
        }
        let existing_cost = self
            .protocol
            .requests
            .values()
            .filter(|record| {
                record.payload == PayloadLocation::HandlerOwned
                    && record.kind.is_ordered() == ordered
            })
            .try_fold(PayloadCost { items: 0, bytes: 0 }, |total, record| {
                total.checked_add(record.cost)
            });
        let handler_cost_bound = if ordered {
            self.protocol.limits.ordered_handler_cost
        } else {
            self.protocol.limits.handler_cost
        };
        if existing_cost
            .and_then(|total| total.checked_add(record.cost))
            .is_none_or(|total| !total.fits(handler_cost_bound))
        {
            return SystemDisposition::RequestUnavailable(request);
        }
        let Some(record) = self.protocol.requests.get_mut(&request) else {
            return SystemDisposition::RequestUnavailable(request);
        };
        if record.payload != PayloadLocation::Queued {
            return SystemDisposition::RequestUnavailable(request);
        }
        record.payload = PayloadLocation::HandlerOwned;
        SystemDisposition::Dispatched(request)
    }

    fn finish(&mut self, request: RequestId, send_response: bool) -> SystemDisposition {
        let Some(record) = self.protocol.requests.get(&request) else {
            return SystemDisposition::RequestUnavailable(request);
        };
        if record.payload != PayloadLocation::HandlerOwned {
            return SystemDisposition::RequestUnavailable(request);
        }
        let response = match record.response {
            None => ResponseResult::NotApplicable,
            Some(ResponseEndpoint::Attached) if send_response => ResponseResult::Sent,
            Some(ResponseEndpoint::Attached | ResponseEndpoint::Abandoned) => {
                ResponseResult::Dropped
            }
        };
        self.protocol.requests.remove(&request);
        SystemDisposition::Finished { request, response }
    }

    fn abandon_receiver(&mut self, request: RequestId) -> SystemDisposition {
        let Some(record) = self.protocol.requests.get_mut(&request) else {
            return SystemDisposition::RequestUnavailable(request);
        };
        let Some(response) = &mut record.response else {
            return SystemDisposition::RequestUnavailable(request);
        };
        if *response != ResponseEndpoint::Attached {
            return SystemDisposition::RequestUnavailable(request);
        }
        *response = ResponseEndpoint::Abandoned;
        SystemDisposition::ReceiverAbandoned(request)
    }

    fn assemble(
        &mut self,
        limits: ModelLimits,
        view: ViewId,
        rules: RulesId,
        succeed: bool,
    ) -> SystemDisposition {
        if self.lifecycle != Lifecycle::Constructing {
            return SystemDisposition::KernelUnavailable;
        }
        if !succeed {
            self.lifecycle = Lifecycle::StartupFailed;
            self.terminalize_requests();
            return SystemDisposition::StartupFailed;
        }
        let Ok(limits) = limits.validate() else {
            self.lifecycle = Lifecycle::StartupFailed;
            self.terminalize_requests();
            return SystemDisposition::StartupFailed;
        };
        self.authority = Some(Omega::new(limits, view, rules));
        self.derived = DerivedState {
            template: DerivedComponent::Enabled {
                source: 0,
                published: 0,
            },
            recent_reject: DerivedHealth::Enabled,
        };
        self.lifecycle = Lifecycle::Initializing;
        SystemDisposition::Assembled
    }

    fn ready(&mut self) -> SystemDisposition {
        if self.lifecycle != Lifecycle::Initializing {
            return SystemDisposition::KernelUnavailable;
        }
        self.lifecycle = Lifecycle::Running;
        SystemDisposition::Running
    }

    fn initialization_replay_failed(&mut self) -> SystemDisposition {
        if self.lifecycle != Lifecycle::Initializing {
            return SystemDisposition::KernelUnavailable;
        }
        self.lifecycle = Lifecycle::Draining;
        self.terminalize_queued_requests();
        SystemDisposition::InitializationDraining
    }

    fn begin_drain(&mut self) -> SystemDisposition {
        if !matches!(self.lifecycle, Lifecycle::Initializing | Lifecycle::Running) {
            return SystemDisposition::KernelUnavailable;
        }
        self.lifecycle = Lifecycle::Draining;
        self.terminalize_queued_requests();
        SystemDisposition::Draining
    }

    fn finish_drain(&mut self) -> SystemDisposition {
        if self.lifecycle != Lifecycle::Draining {
            return SystemDisposition::KernelUnavailable;
        }
        let Some(authority) = &self.authority else {
            return SystemDisposition::KernelUnavailable;
        };
        let handlers = self
            .protocol
            .requests
            .values()
            .any(|record| record.payload == PayloadLocation::HandlerOwned);
        if handlers
            || !authority.linear.work.is_empty()
            || !authority.linear.finished_work.is_empty()
            || !authority.linear.direct_work.is_empty()
            || authority.linear.effect_claim.is_some()
            || authority.has_pending_effects()
        {
            return SystemDisposition::DrainPending;
        }
        self.authority = None;
        self.lifecycle = Lifecycle::Stopped;
        SystemDisposition::Stopped
    }

    fn kernel(&mut self, access: KernelAccess, command: KernelCommand) -> SystemDisposition {
        let allowed = match (self.lifecycle, access) {
            (Lifecycle::Running, KernelAccess::Ordinary) => true,
            (Lifecycle::Initializing, KernelAccess::Initialization) => {
                command.allowed_during_initialization()
            }
            (Lifecycle::Draining, KernelAccess::Drain) => command.allowed_during_drain(),
            _ => false,
        };
        if !allowed {
            return SystemDisposition::KernelUnavailable;
        }
        let Some(authority) = &mut self.authority else {
            return SystemDisposition::KernelUnavailable;
        };
        let step = authority.kernel_step(command);
        if let KernelStep::AuthorityCommit { disposition, .. } = &step {
            let template_changed = match disposition {
                KernelDisposition::Accepted(_)
                | KernelDisposition::AcceptedBatch(_)
                | KernelDisposition::ReplacementAccepted { .. }
                | KernelDisposition::Removed(_)
                | KernelDisposition::ChainReconciled { .. }
                | KernelDisposition::GenerationReplaced { .. } => true,
                KernelDisposition::PeerBanned { removed, .. } => !removed.is_empty(),
                _ => false,
            };
            if template_changed {
                advance_derived_source(&mut self.derived.template);
            }
        }
        SystemDisposition::Kernel(step)
    }

    fn publish_template(&mut self, captured_source: u16) -> SystemDisposition {
        let DerivedComponent::Enabled { source, published } = &mut self.derived.template else {
            return SystemDisposition::DerivedDegraded;
        };
        if *source != captured_source {
            return SystemDisposition::StaleTemplate(captured_source);
        }
        *published = captured_source;
        SystemDisposition::TemplatePublished(captured_source)
    }

    fn terminalize_queued_requests(&mut self) {
        self.protocol
            .requests
            .retain(|_, record| record.payload != PayloadLocation::Queued);
    }

    fn terminalize_requests(&mut self) {
        self.protocol.requests.clear();
    }
}

fn advance_derived_source(component: &mut DerivedComponent) {
    let DerivedComponent::Enabled { source, .. } = component else {
        return;
    };
    if let Some(next) = source.checked_add(1) {
        *source = next;
    } else {
        *component = DerivedComponent::Degraded;
    }
}
