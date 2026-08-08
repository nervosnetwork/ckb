//! Sealed worker topology for the unified tx-pool authority.
//!
//! The service may spawn this worker set but cannot choose raw work permits,
//! verifier capabilities, settlement retry rules, or Ready wake behavior.
//! Once checkout succeeds, the worker owns one linear capability until an
//! authoritative settlement commits or a typed structural fault carrying the
//! capability reaches supervision.

use super::{
    exchange::{AuthorityComputeExecutionPermit, ComputeVerifierSlot, ComputeWorkerSlot},
    plan::{AuthorityFault, ComputeCancellationError, ComputeSettlementRecovery},
    resolver::{ResolutionExecutionKind, ResolutionReceiptDefect, VerificationCacheUpdate},
    runtime::{
        AuthorityComputeAftermath, AuthorityComputeCheckout, AuthorityComputeCompletion,
        AuthorityComputeError, AuthorityComputeOutcome, AuthorityDriverError,
        AuthorityPendingSettlement, AuthorityReadyOutcome, AuthorityRuntime, SettlementOrigin,
    },
    state::{InputEvidenceError, VerifyCapability, WorkPermit},
};
use ckb_async_runtime::Handle;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_verification::cache::TxVerificationCache;
use std::{ops::ControlFlow, sync::Arc, time::Duration};
use tokio::sync::{RwLock, mpsc, watch};

const TRANSIENT_RETRY_DELAY: Duration = Duration::from_millis(1);
const MAINTENANCE_EXPIRY_TICK: Duration = Duration::from_secs(1);

/// Background tasks owned by one authority generation. Their `Result` is
/// intentionally retained so supervision can distinguish clean cancellation
/// from a structural kernel fault before deciding whether persistence is safe.
pub(crate) struct AuthorityWorkerHandles {
    pub(in crate::authority) tasks: Vec<AuthorityWorkerTask>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityWorkerRole {
    Resolver,
    Verifier(usize),
    Ready,
    Maintenance,
}

pub(in crate::authority) struct AuthorityWorkerTask {
    pub(in crate::authority) role: AuthorityWorkerRole,
    pub(in crate::authority) handle: tokio::task::JoinHandle<Result<(), AuthorityWorkerFault>>,
}

#[derive(Debug)]
pub(crate) struct AuthorityWorkerFault {
    kind: AuthorityWorkerFaultKind,
}

#[derive(Debug)]
pub(in crate::authority) enum AuthorityWorkerFaultKind {
    Authority(
        #[cfg_attr(
            not(test),
            expect(
                dead_code,
                reason = "the exact authority fault is retained for the generation diagnostic"
            )
        )]
        AuthorityFault,
    ),
    LifecycleClosed,
    Settlement(
        #[expect(
            dead_code,
            reason = "the unsettled move-only capability is retained until generation shutdown"
        )]
        Box<AuthorityPendingSettlement>,
    ),
}

impl AuthorityWorkerFault {
    fn authority(fault: AuthorityFault) -> Self {
        Self {
            kind: AuthorityWorkerFaultKind::Authority(fault),
        }
    }

    fn lifecycle_closed() -> Self {
        Self {
            kind: AuthorityWorkerFaultKind::LifecycleClosed,
        }
    }

    fn settlement(pending: AuthorityPendingSettlement) -> Self {
        Self {
            kind: AuthorityWorkerFaultKind::Settlement(Box::new(pending)),
        }
    }

    pub(in crate::authority) fn into_kind(self) -> AuthorityWorkerFaultKind {
        self.kind
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityWorkerSpawnError {
    Allocation,
}

enum WorkerRole {
    OrderedResolve {
        slot: ComputeWorkerSlot,
    },
    Verifier {
        slot: ComputeVerifierSlot,
        cache: Arc<RwLock<TxVerificationCache>>,
        cache_updates: mpsc::Sender<VerificationCacheUpdate>,
    },
}

impl WorkerRole {
    fn slot(&self) -> ComputeWorkerSlot {
        match self {
            Self::OrderedResolve { slot } => *slot,
            Self::Verifier { slot, .. } => (*slot).into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerStep {
    Progress,
    WaitForRunnable,
    WaitForEffectCapacity,
    Backoff,
}

/// Lock-free scheduling intent derived from the exact level that woke a
/// worker. `Probe` is used only at startup and after progress so preexisting
/// work remains level-triggered; every wake path names its compatible first
/// checkout and cannot spend the baton on another lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckoutIntent {
    Probe,
    Resolve,
    VerifySmall,
    VerifyAny,
}

struct ComputeWorker {
    runtime: AuthorityRuntime,
    role: WorkerRole,
}

impl AuthorityRuntime {
    /// Spawn the only legal compute topology: one ordered resolver, exactly
    /// the configured verifier count, and one Ready driver. The first of
    /// multiple verifiers is small-cycle-only, preserving the established
    /// starvation boundary; every other verifier accepts either cycle class.
    pub(crate) fn spawn_workers(
        &self,
        handle: &Handle,
        cache: Arc<RwLock<TxVerificationCache>>,
        cache_updates: mpsc::Sender<VerificationCacheUpdate>,
        command_rx: watch::Receiver<ChunkCommand>,
        cancel: CancellationToken,
    ) -> Result<AuthorityWorkerHandles, AuthorityWorkerSpawnError> {
        let worker_count = self.verify_worker_count();
        let task_count = worker_count
            .checked_add(3)
            .ok_or(AuthorityWorkerSpawnError::Allocation)?;
        let mut tasks = Vec::new();
        tasks
            .try_reserve(task_count)
            .map_err(|_| AuthorityWorkerSpawnError::Allocation)?;
        tasks.push(AuthorityWorkerTask {
            role: AuthorityWorkerRole::Resolver,
            handle: handle.spawn(
                ComputeWorker {
                    runtime: self.clone(),
                    role: WorkerRole::OrderedResolve {
                        slot: ComputeWorkerSlot::ordered_resolve(),
                    },
                }
                .run(command_rx.clone(), cancel.child_token()),
            ),
        });
        for worker_id in 0..worker_count {
            let capability = if worker_id == 0 && worker_count > 1 {
                VerifyCapability::SmallCycleOnly
            } else {
                VerifyCapability::Any
            };
            let slot = ComputeVerifierSlot::new(worker_id, capability);
            tasks.push(AuthorityWorkerTask {
                role: AuthorityWorkerRole::Verifier(slot.worker_id()),
                handle: handle.spawn(
                    ComputeWorker {
                        runtime: self.clone(),
                        role: WorkerRole::Verifier {
                            slot,
                            cache: Arc::clone(&cache),
                            cache_updates: cache_updates.clone(),
                        },
                    }
                    .run(command_rx.clone(), cancel.child_token()),
                ),
            });
        }
        let runtime = self.clone();
        let ready_cancel = cancel.child_token();
        tasks.push(AuthorityWorkerTask {
            role: AuthorityWorkerRole::Ready,
            handle: handle.spawn(async move { run_ready_driver(runtime, ready_cancel).await }),
        });
        let runtime = self.clone();
        let maintenance_cancel = cancel.child_token();
        tasks.push(AuthorityWorkerTask {
            role: AuthorityWorkerRole::Maintenance,
            handle: handle
                .spawn(async move { run_maintenance_driver(runtime, maintenance_cancel).await }),
        });
        Ok(AuthorityWorkerHandles { tasks })
    }
}

impl ComputeWorker {
    async fn run(
        self,
        mut command_rx: watch::Receiver<ChunkCommand>,
        cancel: CancellationToken,
    ) -> Result<(), AuthorityWorkerFault> {
        let mut intent = CheckoutIntent::Probe;
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            if !is_resumed(&command_rx) {
                if !wait_for_resume(&mut command_rx, &cancel).await {
                    return Ok(());
                }
                continue;
            }

            // Subscribe to every compatible committed level before checkout.
            // If the probe observes no work, a concurrent Apply has either
            // stored one exact Notify permit or the level remains visible to
            // the next probe. Effect capacity stays a separate heterogeneous
            // releaser and is awaited only after its typed outcome.
            let resolve_notified = self.runtime.resolve_signal().notified();
            let verify_small_notified = self.runtime.verify_small_signal().notified();
            let verify_any_notified = self.runtime.verify_any_signal().notified();
            let capacity_notified = self.runtime.effect_capacity_signal().notified();
            let execution = match self.runtime.acquire_compute_execution(&cancel).await {
                Some(execution) => execution,
                None => return Ok(()),
            };
            let step = self
                .try_process_one(&mut command_rx, execution, intent)
                .await?;
            intent = CheckoutIntent::Probe;
            if step == WorkerStep::Progress {
                continue;
            }

            match step {
                WorkerStep::WaitForRunnable => {
                    let work = async {
                        match &self.role {
                            WorkerRole::OrderedResolve { .. } => {
                                resolve_notified.await;
                                CheckoutIntent::Resolve
                            }
                            WorkerRole::Verifier { slot, .. }
                                if ComputeWorkerSlot::from(*slot).verify_capability()
                                    == Some(VerifyCapability::SmallCycleOnly) =>
                            {
                                tokio::select! {
                                    biased;
                                    _ = verify_small_notified => CheckoutIntent::VerifySmall,
                                    _ = resolve_notified => CheckoutIntent::Resolve,
                                }
                            }
                            WorkerRole::Verifier { .. } => {
                                tokio::select! {
                                    biased;
                                    _ = verify_any_notified => CheckoutIntent::VerifyAny,
                                    _ = verify_small_notified => CheckoutIntent::VerifySmall,
                                    _ = resolve_notified => CheckoutIntent::Resolve,
                                }
                            }
                        }
                    };
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        changed = command_rx.changed() => {
                            if changed.is_err() {
                                return Ok(());
                            }
                        }
                        next = work => intent = next,
                    }
                }
                WorkerStep::WaitForEffectCapacity => {
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        changed = command_rx.changed() => {
                            if changed.is_err() {
                                return Ok(());
                            }
                        }
                        _ = capacity_notified => {}
                    }
                }
                WorkerStep::Backoff => {
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        changed = command_rx.changed() => {
                            if changed.is_err() {
                                return Ok(());
                            }
                        }
                        _ = tokio::time::sleep(TRANSIENT_RETRY_DELAY) => {}
                    }
                }
                WorkerStep::Progress => {}
            }
        }
    }

    async fn try_process_one(
        &self,
        command_rx: &mut watch::Receiver<ChunkCommand>,
        execution: AuthorityComputeExecutionPermit,
        intent: CheckoutIntent,
    ) -> Result<WorkerStep, AuthorityWorkerFault> {
        let job = match self.checkout(execution, intent) {
            Ok(ControlFlow::Continue(Some(job))) => job,
            Ok(ControlFlow::Continue(None)) => return Ok(WorkerStep::WaitForRunnable),
            Ok(ControlFlow::Break(pending)) => return self.recover_settlement(pending).await,
            Err(error) => return self.handle_runtime_error(error).await,
        };
        match self.runtime.execute_compute(job) {
            Ok(AuthorityComputeOutcome::Completion(completion)) => {
                self.commit_completion(completion).await
            }
            Ok(AuthorityComputeOutcome::Verification(request)) => {
                let cache = match &self.role {
                    WorkerRole::Verifier { cache, .. } => cache,
                    WorkerRole::OrderedResolve { .. } => {
                        if let ControlFlow::Break(pending) =
                            self.runtime.retry_unexpected_verification(request)
                        {
                            return self.recover_settlement(pending).await;
                        }
                        // `ResolveOnly` cannot legally issue verification.
                        // The exact capability has already been returned, so
                        // supervision receives a capability-free structural
                        // scheduler fault instead of an unbounded retry loop.
                        return Err(AuthorityWorkerFault::authority(
                            AuthorityFault::SchedulerProjection,
                        ));
                    }
                };
                let request = {
                    let guard = cache.read().await;
                    request.bind_cache(&guard)
                };
                let completion = self
                    .runtime
                    .execute_verification(request, Some(command_rx))
                    .await;
                self.commit_completion(completion).await
            }
            Err(error) => self.handle_runtime_error(error).await,
        }
    }

    async fn commit_completion(
        &self,
        completion: AuthorityComputeCompletion,
    ) -> Result<WorkerStep, AuthorityWorkerFault> {
        let finished = completion.finish_execution();
        match self.runtime.settle_finished(finished) {
            ControlFlow::Continue(aftermath) => self.classify_aftermath(aftermath),
            ControlFlow::Break(pending) => self.recover_settlement(pending).await,
        }
    }

    fn classify_aftermath(
        &self,
        aftermath: AuthorityComputeAftermath,
    ) -> Result<WorkerStep, AuthorityWorkerFault> {
        let (origin, cache_update) = aftermath.into_parts();
        if let Some(update) = cache_update {
            let WorkerRole::Verifier { cache_updates, .. } = &self.role else {
                return Err(AuthorityWorkerFault::authority(
                    AuthorityFault::SchedulerProjection,
                ));
            };
            // Cache publication is deliberately best effort and happens only
            // after authoritative settlement. Dropping a full/closed update
            // cannot change ownership or validation semantics.
            let _ = cache_updates.try_send(update);
        }
        classify_settled_origin(origin)
    }

    fn checkout(
        &self,
        execution: AuthorityComputeExecutionPermit,
        intent: CheckoutIntent,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, Option<super::runtime::AuthorityComputeJob>>,
        AuthorityComputeError,
    > {
        let slot = self.role.slot();
        match &self.role {
            WorkerRole::OrderedResolve { .. } => {
                self.checkout_exact(slot.primary_permit(), execution)
            }
            WorkerRole::Verifier { .. } => {
                let Some(capability) = slot.verify_capability() else {
                    return Err(AuthorityComputeError::Fault(
                        AuthorityFault::SchedulerProjection,
                    ));
                };
                match intent {
                    CheckoutIntent::Probe => match self
                        .runtime
                        .try_checkout(slot.primary_permit(), execution)?
                    {
                        ControlFlow::Break(pending) => Ok(ControlFlow::Break(pending)),
                        ControlFlow::Continue(AuthorityComputeCheckout::Job(job)) => {
                            Ok(ControlFlow::Continue(Some(job)))
                        }
                        ControlFlow::Continue(AuthorityComputeCheckout::Idle(execution)) => self
                            .checkout_exact(
                                slot.fallback_permit().ok_or(AuthorityComputeError::Fault(
                                    AuthorityFault::SchedulerProjection,
                                ))?,
                                execution,
                            ),
                    },
                    CheckoutIntent::Resolve => self.checkout_exact(
                        slot.fallback_permit().ok_or(AuthorityComputeError::Fault(
                            AuthorityFault::SchedulerProjection,
                        ))?,
                        execution,
                    ),
                    CheckoutIntent::VerifySmall => self.checkout_exact(
                        WorkPermit::VerifyOnly(VerifyCapability::SmallCycleOnly),
                        execution,
                    ),
                    CheckoutIntent::VerifyAny => {
                        // Only an Any verifier subscribes to this signal. If
                        // a future topology violates that constructor rule,
                        // retain the worker's narrower capability rather than
                        // allowing it to execute large work.
                        self.checkout_exact(WorkPermit::VerifyOnly(capability), execution)
                    }
                }
            }
        }
    }

    fn checkout_exact(
        &self,
        permit: WorkPermit,
        execution: AuthorityComputeExecutionPermit,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, Option<super::runtime::AuthorityComputeJob>>,
        AuthorityComputeError,
    > {
        match self.runtime.try_checkout(permit, execution)? {
            ControlFlow::Break(pending) => Ok(ControlFlow::Break(pending)),
            ControlFlow::Continue(AuthorityComputeCheckout::Job(job)) => {
                Ok(ControlFlow::Continue(Some(job)))
            }
            ControlFlow::Continue(AuthorityComputeCheckout::Idle(execution)) => {
                drop(execution);
                Ok(ControlFlow::Continue(None))
            }
        }
    }

    async fn handle_runtime_error(
        &self,
        error: AuthorityComputeError,
    ) -> Result<WorkerStep, AuthorityWorkerFault> {
        match error {
            AuthorityComputeError::Allocation => Ok(WorkerStep::Backoff),
            AuthorityComputeError::EffectCapacity => Ok(WorkerStep::WaitForEffectCapacity),
            AuthorityComputeError::LifecycleClosed => Err(AuthorityWorkerFault::lifecycle_closed()),
            AuthorityComputeError::Fault(fault) => Err(AuthorityWorkerFault::authority(fault)),
            AuthorityComputeError::Resolution(kind) => classify_resolution_kind(kind),
        }
    }

    async fn recover_settlement(
        &self,
        pending: AuthorityPendingSettlement,
    ) -> Result<WorkerStep, AuthorityWorkerFault> {
        let (mut failure, aftermath) = pending.into_parts();
        let origin = aftermath.origin();
        loop {
            match failure.recovery() {
                ComputeSettlementRecovery::Obsolete(_) => {
                    // Exact entry-version or phase mismatch proves this
                    // capability no longer names an active Computing owner.
                    drop(failure);
                    return classify_settled_origin(origin);
                }
                ComputeSettlementRecovery::CancelAfterAllocation => {
                    let cancellation = failure.discard_result_for_cancellation();
                    let cancelled = self.runtime.cancel_compute_after_allocation(cancellation);
                    return classify_compute_cancellation(cancelled, origin);
                }
                ComputeSettlementRecovery::WaitEffectCapacity => {}
                ComputeSettlementRecovery::Structural(_) => {
                    return Err(AuthorityWorkerFault::settlement(
                        AuthorityPendingSettlement::from_completion_failure(failure, aftermath),
                    ));
                }
            }

            let notified = self.runtime.effect_capacity_signal().notified();
            let settlement = failure.into_settlement();
            match self.runtime.settle(settlement) {
                Ok(()) => {
                    return self.classify_aftermath(aftermath);
                }
                Err(next) => failure = next,
            }
            // Classify the result of the retry, not the error that preceded
            // it. A structural failure discovered while retrying must reach
            // supervision immediately rather than sleep behind an obsolete
            // backpressure reason.
            match failure.recovery() {
                ComputeSettlementRecovery::WaitEffectCapacity => notified.await,
                ComputeSettlementRecovery::Obsolete(_)
                | ComputeSettlementRecovery::CancelAfterAllocation
                | ComputeSettlementRecovery::Structural(_) => {}
            }
        }
    }
}

fn classify_compute_cancellation(
    result: Result<(), ComputeCancellationError>,
    origin: SettlementOrigin,
) -> Result<WorkerStep, AuthorityWorkerFault> {
    match result {
        // The capability is safely discharged, but the failed result made no
        // forward progress. Back off without owning authority state before
        // the same owner can be checked out and recomputed.
        Ok(()) => Ok(WorkerStep::Backoff),
        Err(ComputeCancellationError::Obsolete(_)) => classify_settled_origin(origin),
        Err(ComputeCancellationError::Fault(fault)) => Err(AuthorityWorkerFault::authority(fault)),
        Err(ComputeCancellationError::EffectClosed) => {
            Err(AuthorityWorkerFault::lifecycle_closed())
        }
    }
}

async fn run_ready_driver(
    runtime: AuthorityRuntime,
    cancel: CancellationToken,
) -> Result<(), AuthorityWorkerFault> {
    loop {
        let work_notified = runtime.ready_signal().notified();
        let capacity_notified = runtime.effect_capacity_signal().notified();
        let step = match runtime.try_drive_ready() {
            Ok(AuthorityReadyOutcome::Applied) => WorkerStep::Progress,
            Ok(AuthorityReadyOutcome::Idle) => WorkerStep::WaitForRunnable,
            Err(error) => classify_driver_error(error)?,
        };
        if step == WorkerStep::Progress {
            continue;
        }
        let wait = async {
            match step {
                WorkerStep::WaitForRunnable => work_notified.await,
                WorkerStep::WaitForEffectCapacity => capacity_notified.await,
                WorkerStep::Backoff => tokio::time::sleep(TRANSIENT_RETRY_DELAY).await,
                WorkerStep::Progress => {}
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = wait => {}
        }
    }
}

/// Drain the three authoritative maintenance levels in bounded fair rounds.
/// The driver owns no queue, cursor or population mirror: every round asks the
/// authority for at most one configured Remote slice, one Accepted root and
/// one dependency edge/marker. A wake is only a hint and is subscribed before
/// the level reads; the timer exists solely because wall-clock expiry can
/// become due without an authority mutation.
pub(in crate::authority) async fn run_maintenance_driver(
    runtime: AuthorityRuntime,
    cancel: CancellationToken,
) -> Result<(), AuthorityWorkerFault> {
    run_maintenance_driver_loop(runtime, cancel, || {}).await
}

async fn run_maintenance_driver_loop(
    runtime: AuthorityRuntime,
    cancel: CancellationToken,
    mut observe_round: impl FnMut(),
) -> Result<(), AuthorityWorkerFault> {
    let mut expiry = tokio::time::interval(MAINTENANCE_EXPIRY_TICK);
    expiry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Tokio intervals expose one immediate setup tick. Consume it before the
    // loop so the first wall-clock maintenance wake remains one period away
    // without manually computing an overflow-capable deadline.
    expiry.tick().await;

    let result = async {
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }

            // Subscribe before reading every level. A relevant Apply between
            // the last Idle result and suspension is therefore observed,
            // while coalescing repeated hints cannot lose authoritative work.
            let work_notified = runtime.maintenance_signal().notified();
            let capacity_notified = runtime.effect_capacity_signal().notified();
            observe_round();
            runtime.publish_operational_metrics();
            let step = run_maintenance_round(&runtime)?;
            match step {
                WorkerStep::Progress => {
                    tokio::task::yield_now().await;
                }
                WorkerStep::WaitForRunnable => {
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = work_notified => {}
                        _ = expiry.tick() => {}
                    }
                }
                WorkerStep::WaitForEffectCapacity => {
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = capacity_notified => {}
                    }
                }
                WorkerStep::Backoff => {
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(TRANSIENT_RETRY_DELAY) => {}
                    }
                }
            }
        }
    }
    .await;
    crate::metrics::OperationalMetrics::default().publish();
    result
}

fn run_maintenance_round(runtime: &AuthorityRuntime) -> Result<WorkerStep, AuthorityWorkerFault> {
    let remote = classify_maintenance_result(runtime.expire_remote_due())?;
    let accepted = classify_maintenance_result(runtime.expire_accepted_due())?;
    let dependency = classify_maintenance_result(runtime.maintain_dependency())?;
    Ok(merge_maintenance_steps(
        merge_maintenance_steps(remote, accepted),
        dependency,
    ))
}

fn classify_maintenance_result(
    result: Result<super::runtime::AuthorityMaintenanceOutcome, AuthorityDriverError>,
) -> Result<WorkerStep, AuthorityWorkerFault> {
    match result {
        Ok(super::runtime::AuthorityMaintenanceOutcome::Applied) => Ok(WorkerStep::Progress),
        Ok(super::runtime::AuthorityMaintenanceOutcome::Idle) => Ok(WorkerStep::WaitForRunnable),
        Err(error) => classify_driver_error(error),
    }
}

fn merge_maintenance_steps(left: WorkerStep, right: WorkerStep) -> WorkerStep {
    match (left, right) {
        (WorkerStep::Progress, _) | (_, WorkerStep::Progress) => WorkerStep::Progress,
        (WorkerStep::Backoff, _) | (_, WorkerStep::Backoff) => WorkerStep::Backoff,
        (WorkerStep::WaitForEffectCapacity, _) | (_, WorkerStep::WaitForEffectCapacity) => {
            WorkerStep::WaitForEffectCapacity
        }
        (WorkerStep::WaitForRunnable, WorkerStep::WaitForRunnable) => WorkerStep::WaitForRunnable,
    }
}

fn classify_resolution_kind(
    kind: ResolutionExecutionKind,
) -> Result<WorkerStep, AuthorityWorkerFault> {
    match kind {
        ResolutionExecutionKind::StaleView | ResolutionExecutionKind::ComputeBudget => {
            Ok(WorkerStep::Progress)
        }
        ResolutionExecutionKind::ResourceUnavailable => Ok(WorkerStep::Backoff),
        ResolutionExecutionKind::InvalidReceipt(error) => Err(AuthorityWorkerFault::authority(
            resolution_receipt_fault(error),
        )),
    }
}

fn resolution_receipt_fault(error: ResolutionReceiptDefect) -> AuthorityFault {
    match error {
        ResolutionReceiptDefect::TransactionMismatch => AuthorityFault::MembershipProjection,
        ResolutionReceiptDefect::EmptyDependencies => AuthorityFault::DependencyProjection,
        ResolutionReceiptDefect::InvalidEvidence(error) => match error {
            InputEvidenceError::Footprint(_) | InputEvidenceError::ResidentBelowSerialized => {
                AuthorityFault::ResourceProjection
            }
            InputEvidenceError::DependencySet(_) => AuthorityFault::DependencyProjection,
        },
    }
}

fn classify_settled_origin(origin: SettlementOrigin) -> Result<WorkerStep, AuthorityWorkerFault> {
    match origin {
        SettlementOrigin::Completion => Ok(WorkerStep::Progress),
        SettlementOrigin::Capture(kind) | SettlementOrigin::Resolution(kind) => {
            classify_resolution_kind(kind)
        }
    }
}

fn classify_driver_error(error: AuthorityDriverError) -> Result<WorkerStep, AuthorityWorkerFault> {
    match error {
        AuthorityDriverError::Stale => Ok(WorkerStep::Progress),
        AuthorityDriverError::Allocation => Ok(WorkerStep::Backoff),
        AuthorityDriverError::EffectCapacity => Ok(WorkerStep::WaitForEffectCapacity),
        AuthorityDriverError::LifecycleClosed => Err(AuthorityWorkerFault::lifecycle_closed()),
        AuthorityDriverError::Fault(fault) => Err(AuthorityWorkerFault::authority(fault)),
    }
}

fn is_resumed(command_rx: &watch::Receiver<ChunkCommand>) -> bool {
    matches!(&*command_rx.borrow(), ChunkCommand::Resume)
}

async fn wait_for_resume(
    command_rx: &mut watch::Receiver<ChunkCommand>,
    cancel: &CancellationToken,
) -> bool {
    loop {
        if is_resumed(command_rx) {
            return true;
        }
        tokio::select! {
            _ = cancel.cancelled() => return false,
            changed = command_rx.changed() => {
                if changed.is_err() {
                    return false;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/worker_policy.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/support/worker.rs"]
pub(in crate::authority) mod test_support;
