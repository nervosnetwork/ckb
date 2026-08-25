//! Sealed worker topology for the unified tx-pool authority.
//!
//! The service may spawn this worker set but cannot choose raw work permits,
//! verifier capabilities, settlement retry rules, or Ready wake behavior.
//! Once checkout succeeds, the worker owns one linear capability until an
//! authoritative settlement commits or a typed structural fault carrying the
//! capability reaches supervision.

use super::{
    compute_coordinator::spawn_compute_exchange,
    plan::AuthorityFault,
    resolver::VerificationCacheUpdate,
    runtime::{
        AuthorityDriverError, AuthorityGenerationReplacementError, AuthorityPendingSettlement,
        AuthorityReadyOutcome, AuthorityRuntime,
    },
};
use ckb_async_runtime::Handle;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_verification::cache::TxVerificationCache;
use std::{sync::Arc, time::Duration};
use tokio::sync::{RwLock, mpsc, watch};

const MAINTENANCE_EXPIRY_TICK: Duration = Duration::from_secs(1);

/// Background tasks owned by one authority generation. Their `Result` is
/// intentionally retained so supervision can distinguish clean cancellation
/// from a structural kernel fault before deciding whether persistence is safe.
pub(crate) struct AuthorityWorkerHandles {
    pub(in crate::authority) tasks: Vec<AuthorityWorkerTask>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityWorkerRole {
    ComputeCoordinator,
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
        #[expect(
            dead_code,
            reason = "the exact authority fault is retained for the generation diagnostic"
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
    Completion(
        #[cfg_attr(
            not(test),
            expect(
                dead_code,
                reason = "the stranded completion capability is retained until generation shutdown"
            )
        )]
        Box<crate::authority::plan::ComputeExchangeCompletion>,
    ),
    Exchange(
        #[cfg_attr(
            not(test),
            expect(
                dead_code,
                reason = "the failed exchange capabilities are retained until generation shutdown"
            )
        )]
        Box<crate::authority::plan::ComputeExchangePlanFailure>,
    ),
}

impl AuthorityWorkerFault {
    pub(in crate::authority) fn authority(fault: AuthorityFault) -> Self {
        Self {
            kind: AuthorityWorkerFaultKind::Authority(fault),
        }
    }

    pub(in crate::authority) fn lifecycle_closed() -> Self {
        Self {
            kind: AuthorityWorkerFaultKind::LifecycleClosed,
        }
    }

    pub(in crate::authority) fn settlement(pending: AuthorityPendingSettlement) -> Self {
        Self {
            kind: AuthorityWorkerFaultKind::Settlement(Box::new(pending)),
        }
    }

    pub(in crate::authority) fn completion(
        completion: crate::authority::plan::ComputeExchangeCompletion,
    ) -> Self {
        Self {
            kind: AuthorityWorkerFaultKind::Completion(Box::new(completion)),
        }
    }

    pub(in crate::authority) fn exchange(
        failure: crate::authority::plan::ComputeExchangePlanFailure,
    ) -> Self {
        Self {
            kind: AuthorityWorkerFaultKind::Exchange(Box::new(failure)),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerStep {
    Progress,
    WaitForRunnable,
    WaitForEffectCapacity,
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
        let mut tasks = spawn_compute_exchange(
            handle,
            self,
            cache,
            cache_updates,
            command_rx,
            cancel.child_token(),
        )?;
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

async fn run_ready_driver(
    runtime: AuthorityRuntime,
    cancel: CancellationToken,
) -> Result<(), AuthorityWorkerFault> {
    run_ready_driver_loop(runtime, cancel, || {}).await
}

async fn run_ready_driver_loop(
    runtime: AuthorityRuntime,
    cancel: CancellationToken,
    mut observe_attempt: impl FnMut(),
) -> Result<(), AuthorityWorkerFault> {
    let mut continuation = None;
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let work_notified = runtime.ready_signal().notified();
        let capacity_notified = runtime.effect_capacity_signal().notified();
        tokio::pin!(capacity_notified);
        let _ = capacity_notified.as_mut().enable();
        observe_attempt();
        let result = match continuation.take() {
            Some(continuation) => runtime.resume_ready(continuation),
            None => runtime.try_drive_ready(),
        };
        let step = match result {
            Ok(AuthorityReadyOutcome::Applied) => WorkerStep::Progress,
            Ok(AuthorityReadyOutcome::Idle) => WorkerStep::WaitForRunnable,
            Ok(AuthorityReadyOutcome::EffectCapacity(blocked)) => {
                continuation = Some(blocked);
                WorkerStep::WaitForEffectCapacity
            }
            Err(error) => classify_driver_error(&runtime, error)?,
        };
        if step == WorkerStep::Progress {
            // One attempt owns at most one bounded Ready Apply or one changed
            // OCC cut. Relinquish the executor before observing another cut,
            // so continuous Ready input cannot hide cancellation or peers.
            tokio::task::yield_now().await;
            continue;
        }
        let wait = async {
            match step {
                WorkerStep::WaitForRunnable => work_notified.await,
                WorkerStep::WaitForEffectCapacity => capacity_notified.as_mut().await,
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
            tokio::pin!(capacity_notified);
            let _ = capacity_notified.as_mut().enable();
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
                        _ = capacity_notified.as_mut() => {}
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
    let remote = classify_maintenance_result(runtime, runtime.expire_remote_due())?;
    let accepted = classify_maintenance_result(runtime, runtime.expire_accepted_due())?;
    let dependency = classify_maintenance_result(runtime, runtime.maintain_dependency())?;
    Ok(merge_maintenance_steps(
        merge_maintenance_steps(remote, accepted),
        dependency,
    ))
}

fn classify_maintenance_result(
    runtime: &AuthorityRuntime,
    result: Result<super::runtime::AuthorityMaintenanceOutcome, AuthorityDriverError>,
) -> Result<WorkerStep, AuthorityWorkerFault> {
    match result {
        Ok(super::runtime::AuthorityMaintenanceOutcome::Applied) => Ok(WorkerStep::Progress),
        Ok(super::runtime::AuthorityMaintenanceOutcome::Idle) => Ok(WorkerStep::WaitForRunnable),
        Err(error) => classify_driver_error(runtime, error),
    }
}

fn merge_maintenance_steps(left: WorkerStep, right: WorkerStep) -> WorkerStep {
    match (left, right) {
        (WorkerStep::Progress, _) | (_, WorkerStep::Progress) => WorkerStep::Progress,
        (WorkerStep::WaitForEffectCapacity, _) | (_, WorkerStep::WaitForEffectCapacity) => {
            WorkerStep::WaitForEffectCapacity
        }
        (WorkerStep::WaitForRunnable, WorkerStep::WaitForRunnable) => WorkerStep::WaitForRunnable,
    }
}

fn classify_driver_error(
    runtime: &AuthorityRuntime,
    error: AuthorityDriverError,
) -> Result<WorkerStep, AuthorityWorkerFault> {
    match error {
        AuthorityDriverError::Stale => Ok(WorkerStep::Progress),
        AuthorityDriverError::Allocation => {
            runtime
                .replace_current_generation_after_allocation()
                .map_err(map_generation_replacement_error)?;
            Ok(WorkerStep::Progress)
        }
        AuthorityDriverError::EffectCapacity => Ok(WorkerStep::WaitForEffectCapacity),
        AuthorityDriverError::LifecycleClosed => Err(AuthorityWorkerFault::lifecycle_closed()),
        AuthorityDriverError::Fault(fault) => Err(AuthorityWorkerFault::authority(fault)),
    }
}

fn map_generation_replacement_error(
    error: AuthorityGenerationReplacementError,
) -> AuthorityWorkerFault {
    match error {
        AuthorityGenerationReplacementError::LifecycleClosed => {
            AuthorityWorkerFault::lifecycle_closed()
        }
        AuthorityGenerationReplacementError::Fault(fault) => AuthorityWorkerFault::authority(fault),
    }
}

#[cfg(test)]
#[path = "tests/support/worker.rs"]
pub(in crate::authority) mod test_support;
