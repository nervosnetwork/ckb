//! Sealed worker topology for the unified tx-pool authority.
//!
//! The service may spawn this worker set but cannot choose raw work permits,
//! verifier capabilities, settlement retry rules, or Ready wake behavior.
//! Once checkout succeeds, the worker owns one linear capability until an
//! authoritative settlement commits or a typed structural fault carrying the
//! capability reaches supervision.

use super::{
    plan::{Backpressure, PlanError},
    resolver::{ResolutionExecutionKind, VerificationCacheUpdate},
    runtime::{
        AuthorityComputeCheckout, AuthorityComputeExecutionPermit, AuthorityComputeOutcome,
        AuthorityPendingSettlement, AuthorityReadyOutcome, AuthorityRuntime, AuthorityRuntimeError,
        FinalAdmissionCaptureError, ReadyValidationError, SettlementOrigin,
    },
    state::{VerifyCapability, WorkPermit},
    validation::FinalAdmissionValidationError,
};
use ckb_async_runtime::Handle;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_verification::cache::TxVerificationCache;
use std::{future::pending, ops::ControlFlow, sync::Arc, time::Duration};
use tokio::sync::{Notify, RwLock, mpsc, watch};

const TRANSIENT_RETRY_DELAY: Duration = Duration::from_millis(1);

/// Background tasks owned by one authority generation. Their `Result` is
/// intentionally retained so supervision can distinguish clean cancellation
/// from a structural kernel fault before deciding whether persistence is safe.
pub(crate) struct AuthorityWorkerHandles {
    pub(crate) resolver: tokio::task::JoinHandle<Result<(), AuthorityWorkerFault>>,
    pub(crate) verifiers: Vec<tokio::task::JoinHandle<Result<(), AuthorityWorkerFault>>>,
    pub(crate) ready: tokio::task::JoinHandle<Result<(), AuthorityWorkerFault>>,
}

#[derive(Debug)]
pub(crate) struct AuthorityWorkerFault {
    kind: AuthorityWorkerFaultKind,
}

#[derive(Debug)]
enum AuthorityWorkerFaultKind {
    Runtime(AuthorityRuntimeError),
    Settlement(Box<AuthorityPendingSettlement>),
    UnexpectedVerificationLane,
}

impl AuthorityWorkerFault {
    fn runtime(error: AuthorityRuntimeError) -> Self {
        Self {
            kind: AuthorityWorkerFaultKind::Runtime(error),
        }
    }

    fn unexpected_verification_lane() -> Self {
        Self {
            kind: AuthorityWorkerFaultKind::UnexpectedVerificationLane,
        }
    }

    fn settlement(pending: AuthorityPendingSettlement) -> Self {
        Self {
            kind: AuthorityWorkerFaultKind::Settlement(Box::new(pending)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityWorkerSpawnError {
    Allocation,
}

enum WorkerRole {
    OrderedResolve,
    Verifier {
        capability: VerifyCapability,
        cache: Arc<RwLock<TxVerificationCache>>,
        cache_updates: mpsc::Sender<VerificationCacheUpdate>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerStep {
    Progress,
    Wait,
    Backoff,
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
        let mut verifiers = Vec::new();
        verifiers
            .try_reserve(worker_count)
            .map_err(|_| AuthorityWorkerSpawnError::Allocation)?;
        let resolver = handle.spawn(
            ComputeWorker {
                runtime: self.clone(),
                role: WorkerRole::OrderedResolve,
            }
            .run(command_rx.clone(), cancel.child_token()),
        );
        for worker_id in 0..worker_count {
            let capability = if worker_id == 0 && worker_count > 1 {
                VerifyCapability::SmallCycleOnly
            } else {
                VerifyCapability::Any
            };
            verifiers.push(
                handle.spawn(
                    ComputeWorker {
                        runtime: self.clone(),
                        role: WorkerRole::Verifier {
                            capability,
                            cache: Arc::clone(&cache),
                            cache_updates: cache_updates.clone(),
                        },
                    }
                    .run(command_rx.clone(), cancel.child_token()),
                ),
            );
        }
        let runtime = self.clone();
        let ready_cancel = cancel.child_token();
        let ready = handle.spawn(async move { run_ready_driver(runtime, ready_cancel).await });
        Ok(AuthorityWorkerHandles {
            resolver,
            verifiers,
            ready,
        })
    }
}

impl ComputeWorker {
    async fn run(
        self,
        mut command_rx: watch::Receiver<ChunkCommand>,
        cancel: CancellationToken,
    ) -> Result<(), AuthorityWorkerFault> {
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

            // Subscribe before reading either authoritative level. A wake is
            // only a hint, but this ordering prevents a mutation between the
            // empty read and suspension from being lost.
            let (primary, secondary) = self.lane_signals();
            let primary_notified = primary.notified();
            let secondary_notified = async {
                match secondary.as_ref() {
                    Some(signal) => signal.notified().await,
                    None => pending().await,
                }
            };
            let execution = match self.runtime.acquire_compute_execution(&cancel).await {
                Ok(Some(execution)) => execution,
                Ok(None) => return Ok(()),
                Err(error) => return Err(AuthorityWorkerFault::runtime(error)),
            };
            let step = self.try_process_one(&mut command_rx, execution).await?;
            if step == WorkerStep::Progress {
                continue;
            }

            let retry_delay = async {
                if step == WorkerStep::Backoff {
                    tokio::time::sleep(TRANSIENT_RETRY_DELAY).await;
                } else {
                    pending().await
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                changed = command_rx.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                }
                _ = primary_notified => {}
                _ = secondary_notified => {}
                _ = retry_delay => {}
            }
        }
    }

    fn lane_signals(&self) -> (Arc<Notify>, Option<Arc<Notify>>) {
        match &self.role {
            WorkerRole::OrderedResolve => (
                self.runtime.signal_for_permit(WorkPermit::ResolveOnly),
                None,
            ),
            WorkerRole::Verifier { capability, .. } => (
                self.runtime
                    .signal_for_permit(WorkPermit::VerifyOnly(*capability)),
                Some(
                    self.runtime
                        .signal_for_permit(WorkPermit::ResolveThenVerify(*capability)),
                ),
            ),
        }
    }

    async fn try_process_one(
        &self,
        command_rx: &mut watch::Receiver<ChunkCommand>,
        execution: AuthorityComputeExecutionPermit,
    ) -> Result<WorkerStep, AuthorityWorkerFault> {
        let job = match self.checkout(execution) {
            Ok(ControlFlow::Continue(Some(job))) => job,
            Ok(ControlFlow::Continue(None)) => return Ok(WorkerStep::Wait),
            Ok(ControlFlow::Break(pending)) => return self.recover_settlement(pending).await,
            Err(error) => return self.handle_runtime_error(error).await,
        };
        match self.runtime.execute_compute(job) {
            Ok(ControlFlow::Continue(AuthorityComputeOutcome::Settled)) => Ok(WorkerStep::Progress),
            Ok(ControlFlow::Continue(AuthorityComputeOutcome::Verification(request))) => {
                let (cache, cache_updates) = match &self.role {
                    WorkerRole::Verifier {
                        cache,
                        cache_updates,
                        ..
                    } => (cache, cache_updates),
                    WorkerRole::OrderedResolve => {
                        if let ControlFlow::Break(pending) =
                            self.runtime.retry_unexpected_verification(request)
                        {
                            self.recover_settlement(pending).await?;
                        }
                        return Err(AuthorityWorkerFault::unexpected_verification_lane());
                    }
                };
                let request = {
                    let guard = cache.read().await;
                    request.bind_cache(&guard)
                };
                match self
                    .runtime
                    .execute_verification(request, Some(command_rx))
                    .await
                {
                    Ok(ControlFlow::Continue(outcome)) => {
                        if let Some(update) = outcome.cache_update {
                            // Cache publication is deliberately best effort
                            // and happens only after authoritative settlement.
                            // Dropping a full/closed update cannot change pool
                            // ownership or validation semantics.
                            let _ = cache_updates.try_send(update);
                        }
                        Ok(WorkerStep::Progress)
                    }
                    Ok(ControlFlow::Break(pending)) => self.recover_settlement(pending).await,
                    Err(error) => self.handle_runtime_error(error).await,
                }
            }
            Ok(ControlFlow::Break(pending)) => self.recover_settlement(pending).await,
            Err(error) => self.handle_runtime_error(error).await,
        }
    }

    fn checkout(
        &self,
        execution: AuthorityComputeExecutionPermit,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, Option<super::runtime::AuthorityComputeJob>>,
        AuthorityRuntimeError,
    > {
        match &self.role {
            WorkerRole::OrderedResolve => {
                match self
                    .runtime
                    .try_checkout(WorkPermit::ResolveOnly, execution)?
                {
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
            WorkerRole::Verifier { capability, .. } => {
                match self
                    .runtime
                    .try_checkout(WorkPermit::VerifyOnly(*capability), execution)?
                {
                    ControlFlow::Break(pending) => Ok(ControlFlow::Break(pending)),
                    ControlFlow::Continue(AuthorityComputeCheckout::Job(job)) => {
                        Ok(ControlFlow::Continue(Some(job)))
                    }
                    ControlFlow::Continue(AuthorityComputeCheckout::Idle(execution)) => match self
                        .runtime
                        .try_checkout(WorkPermit::ResolveThenVerify(*capability), execution)?
                    {
                        ControlFlow::Break(pending) => Ok(ControlFlow::Break(pending)),
                        ControlFlow::Continue(AuthorityComputeCheckout::Job(job)) => {
                            Ok(ControlFlow::Continue(Some(job)))
                        }
                        ControlFlow::Continue(AuthorityComputeCheckout::Idle(execution)) => {
                            drop(execution);
                            Ok(ControlFlow::Continue(None))
                        }
                    },
                }
            }
        }
    }

    async fn handle_runtime_error(
        &self,
        error: AuthorityRuntimeError,
    ) -> Result<WorkerStep, AuthorityWorkerFault> {
        match error {
            AuthorityRuntimeError::Plan(error) => classify_plan_error(error),
            AuthorityRuntimeError::Capture(kind) => classify_resolution_kind(kind, true),
            AuthorityRuntimeError::Execution(kind) => classify_resolution_kind(kind, false),
            AuthorityRuntimeError::Verification(kind) => Err(AuthorityWorkerFault::runtime(
                AuthorityRuntimeError::Verification(kind),
            )),
            AuthorityRuntimeError::ComputeGateClosed => Err(AuthorityWorkerFault::runtime(
                AuthorityRuntimeError::ComputeGateClosed,
            )),
            error @ (AuthorityRuntimeError::FinalCapture(_)
            | AuthorityRuntimeError::ReadyValidation(_)) => {
                Err(AuthorityWorkerFault::runtime(error))
            }
        }
    }

    async fn recover_settlement(
        &self,
        pending: AuthorityPendingSettlement,
    ) -> Result<WorkerStep, AuthorityWorkerFault> {
        let (mut failure, origin, execution) = pending.into_parts();
        loop {
            match failure.error() {
                PlanError::Stale(_) => {
                    // Exact entry version/phase/lease mismatch proves this
                    // capability no longer names an active Computing owner.
                    drop(failure);
                    drop(execution);
                    return classify_settled_origin(origin);
                }
                PlanError::Backpressure(_) => {}
                _ => {
                    return Err(AuthorityWorkerFault::settlement(
                        AuthorityPendingSettlement::new(failure, origin, execution),
                    ));
                }
            }

            let signal = self.runtime.mutation_signal();
            let notified = signal.notified();
            let settlement = failure.into_settlement();
            match self.runtime.settle(settlement) {
                Ok(()) => {
                    drop(execution);
                    return classify_settled_origin(origin);
                }
                Err(next) => failure = next,
            }
            // Classify the result of the retry, not the error that preceded
            // it. A structural failure discovered while retrying must reach
            // supervision immediately rather than sleep behind an obsolete
            // backpressure reason.
            match settlement_wait(failure.error()) {
                WorkerStep::Backoff => {
                    tokio::select! {
                        _ = notified => {}
                        _ = tokio::time::sleep(TRANSIENT_RETRY_DELAY) => {}
                    }
                }
                WorkerStep::Wait => notified.await,
                WorkerStep::Progress => {}
            }
        }
    }
}

async fn run_ready_driver(
    runtime: AuthorityRuntime,
    cancel: CancellationToken,
) -> Result<(), AuthorityWorkerFault> {
    loop {
        let signal = runtime.mutation_signal();
        let notified = signal.notified();
        let step = match runtime.try_drive_ready() {
            Ok(AuthorityReadyOutcome::Applied { .. }) => WorkerStep::Progress,
            Ok(AuthorityReadyOutcome::Idle) => WorkerStep::Wait,
            Err(error) => classify_ready_error(error)?,
        };
        if step == WorkerStep::Progress {
            continue;
        }
        let retry_delay = async {
            if step == WorkerStep::Backoff {
                tokio::time::sleep(TRANSIENT_RETRY_DELAY).await;
            } else {
                pending().await
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = notified => {}
            _ = retry_delay => {}
        }
    }
}

fn classify_plan_error(error: PlanError) -> Result<WorkerStep, AuthorityWorkerFault> {
    match error {
        PlanError::Stale(_) => Ok(WorkerStep::Progress),
        PlanError::Backpressure(Backpressure::Allocation) => Ok(WorkerStep::Backoff),
        PlanError::Backpressure(_) => Ok(WorkerStep::Wait),
        error => Err(AuthorityWorkerFault::runtime(AuthorityRuntimeError::Plan(
            error,
        ))),
    }
}

fn classify_resolution_kind(
    kind: ResolutionExecutionKind,
    capture: bool,
) -> Result<WorkerStep, AuthorityWorkerFault> {
    match kind {
        ResolutionExecutionKind::StaleView | ResolutionExecutionKind::ComputeBudget => {
            Ok(WorkerStep::Progress)
        }
        ResolutionExecutionKind::ResourceUnavailable => Ok(WorkerStep::Backoff),
        ResolutionExecutionKind::InvalidReceipt(_) => {
            let error = if capture {
                AuthorityRuntimeError::Capture(kind)
            } else {
                AuthorityRuntimeError::Execution(kind)
            };
            Err(AuthorityWorkerFault::runtime(error))
        }
    }
}

fn classify_settled_origin(origin: SettlementOrigin) -> Result<WorkerStep, AuthorityWorkerFault> {
    match origin {
        SettlementOrigin::Completion => Ok(WorkerStep::Progress),
        SettlementOrigin::Capture(kind) => classify_resolution_kind(kind, true),
        SettlementOrigin::Resolution(kind) => classify_resolution_kind(kind, false),
        SettlementOrigin::Verification(kind) => Err(AuthorityWorkerFault::runtime(
            AuthorityRuntimeError::Verification(kind),
        )),
    }
}

fn settlement_wait(error: &PlanError) -> WorkerStep {
    match error {
        PlanError::Backpressure(Backpressure::Allocation) => WorkerStep::Backoff,
        PlanError::Backpressure(_) => WorkerStep::Wait,
        _ => WorkerStep::Progress,
    }
}

fn classify_ready_error(error: AuthorityRuntimeError) -> Result<WorkerStep, AuthorityWorkerFault> {
    match error {
        AuthorityRuntimeError::Plan(error) => classify_plan_error(error),
        AuthorityRuntimeError::FinalCapture(error) => match error {
            FinalAdmissionCaptureError::Plan(PlanError::Stale(_))
            | FinalAdmissionCaptureError::Validation(FinalAdmissionValidationError::StaleView) => {
                Ok(WorkerStep::Progress)
            }
            FinalAdmissionCaptureError::Allocation
            | FinalAdmissionCaptureError::Plan(PlanError::Backpressure(Backpressure::Allocation))
            | FinalAdmissionCaptureError::Validation(FinalAdmissionValidationError::Allocation) => {
                Ok(WorkerStep::Backoff)
            }
            FinalAdmissionCaptureError::Plan(PlanError::Backpressure(_)) => Ok(WorkerStep::Wait),
            error => Err(AuthorityWorkerFault::runtime(
                AuthorityRuntimeError::FinalCapture(error),
            )),
        },
        AuthorityRuntimeError::ReadyValidation(error) => match error {
            ReadyValidationError::Candidate(FinalAdmissionValidationError::StaleView) => {
                Ok(WorkerStep::Progress)
            }
            ReadyValidationError::Allocation
            | ReadyValidationError::Candidate(FinalAdmissionValidationError::Allocation) => {
                Ok(WorkerStep::Backoff)
            }
            error => Err(AuthorityWorkerFault::runtime(
                AuthorityRuntimeError::ReadyValidation(error),
            )),
        },
        error => Err(AuthorityWorkerFault::runtime(error)),
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
