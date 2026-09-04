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
        AuthorityDriverError, AuthorityPendingSettlement, AuthorityReadyCommitAssignment,
        AuthorityReadyCommitLane, AuthorityReadyCommitTerminal, AuthorityReadyDispatch,
        AuthorityReadyOutcome, AuthorityRuntime,
    },
};
use crate::constants::MAX_READY_BATCH;
use ckb_async_runtime::Handle;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_verification::cache::TxVerificationCache;
use std::{sync::Arc, time::Duration};
use tokio::sync::{RwLock, mpsc, oneshot, watch};

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
    ReadyCommit(usize),
    Ready,
    Maintenance,
}

struct ReadyCommitWork {
    runtime: AuthorityRuntime,
    terminal: Option<(
        AuthorityReadyCommitAssignment,
        oneshot::Sender<AuthorityReadyCommitTerminal>,
    )>,
}

struct ReadyCommitWorker {
    runtime: AuthorityRuntime,
    lane: AuthorityReadyCommitLane,
    assignments: mpsc::Receiver<ReadyCommitWork>,
}

struct ReadyWaveExecutor {
    runtime: AuthorityRuntime,
    assignments: Vec<mpsc::Sender<ReadyCommitWork>>,
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

impl ReadyCommitWork {
    fn new(
        runtime: &AuthorityRuntime,
        assignment: AuthorityReadyCommitAssignment,
        result: oneshot::Sender<AuthorityReadyCommitTerminal>,
    ) -> Self {
        Self {
            runtime: runtime.clone(),
            terminal: Some((assignment, result)),
        }
    }

    fn take_for_commit(
        &mut self,
    ) -> Option<(
        AuthorityReadyCommitAssignment,
        oneshot::Sender<AuthorityReadyCommitTerminal>,
    )> {
        self.terminal.take()
    }

    fn cancel(mut self) -> Result<(), AuthorityFault> {
        let Some((assignment, result)) = self.terminal.take() else {
            return Err(AuthorityFault::SchedulerProjection);
        };
        drop(result);
        self.runtime.cancel_ready_assignment(assignment)
    }
}

impl Drop for ReadyCommitWork {
    fn drop(&mut self) {
        let Some((assignment, result)) = self.terminal.take() else {
            return;
        };
        drop(result);
        // Receiver/channel loss is already a generation integrity fault. The
        // move-owned job must still release its exact reservation and staged
        // effect, and publish the resulting wake, before that fault reaches
        // supervision. An effect-projection failure remains covered by the
        // same persistence-forbidden worker exit.
        let _ = self.runtime.cancel_ready_assignment(assignment);
    }
}

/// Cancellation guard for compiled jobs in later conflict waves. If the
/// Ready driver is aborted while awaiting an earlier wave, every not-yet-sent
/// job is still explicitly terminalized and its exact wake is published.
struct PendingReadyAssignments {
    runtime: AuthorityRuntime,
    assignments: std::vec::IntoIter<AuthorityReadyCommitAssignment>,
}

impl PendingReadyAssignments {
    fn new(runtime: &AuthorityRuntime, assignments: Vec<AuthorityReadyCommitAssignment>) -> Self {
        Self {
            runtime: runtime.clone(),
            assignments: assignments.into_iter(),
        }
    }

    fn next(&mut self) -> Option<AuthorityReadyCommitAssignment> {
        self.assignments.next()
    }

    fn cancel_remaining(&mut self) -> Option<AuthorityFault> {
        let mut fault = None;
        for assignment in self.assignments.by_ref() {
            if let Err(error) = self.runtime.cancel_ready_assignment(assignment) {
                fault.get_or_insert(error);
            }
        }
        fault
    }
}

impl Drop for PendingReadyAssignments {
    fn drop(&mut self) {
        let _ = self.cancel_remaining();
    }
}

impl ReadyWaveExecutor {
    fn new(
        runtime: &AuthorityRuntime,
    ) -> Result<(Self, Vec<ReadyCommitWorker>), AuthorityWorkerSpawnError> {
        let mut assignments = Vec::new();
        let mut workers = Vec::new();
        assignments
            .try_reserve_exact(MAX_READY_BATCH)
            .map_err(|_| AuthorityWorkerSpawnError::Allocation)?;
        workers
            .try_reserve_exact(MAX_READY_BATCH)
            .map_err(|_| AuthorityWorkerSpawnError::Allocation)?;
        for index in 0..MAX_READY_BATCH {
            let lane = AuthorityReadyCommitLane::from_index(index);
            let (sender, receiver) = mpsc::channel(1);
            assignments.push(sender);
            workers.push(ReadyCommitWorker {
                runtime: runtime.clone(),
                lane,
                assignments: receiver,
            });
        }
        Ok((
            Self {
                runtime: runtime.clone(),
                assignments,
            },
            workers,
        ))
    }

    async fn execute(
        &mut self,
        wave: super::runtime::AuthorityReadyWave,
    ) -> Result<AuthorityReadyOutcome, AuthorityDriverError> {
        let assignments = wave.into_assignments();
        if assignments.is_empty()
            || assignments.len() > MAX_READY_BATCH
            || assignments.len() > self.assignments.len()
        {
            let mut cancel_fault = None;
            for assignment in assignments {
                if let Err(fault) = self.runtime.cancel_ready_assignment(assignment) {
                    cancel_fault.get_or_insert(fault);
                }
            }
            if let Some(fault) = cancel_fault {
                return Err(AuthorityDriverError::Fault(fault));
            }
            return Err(AuthorityDriverError::Fault(
                AuthorityFault::SchedulerProjection,
            ));
        }
        let wave_len = assignments.len();
        let mut pending = PendingReadyAssignments::new(&self.runtime, assignments);
        let mut results = Vec::with_capacity(wave_len);
        let mut transport_closed = false;
        let mut cleanup_fault = None;
        for sender in self.assignments.iter().take(wave_len) {
            let Some(assignment) = pending.next() else {
                if let Some(fault) = pending.cancel_remaining() {
                    return Err(AuthorityDriverError::Fault(fault));
                }
                return Err(AuthorityDriverError::Fault(
                    AuthorityFault::SchedulerProjection,
                ));
            };
            let (result, receiver) = oneshot::channel();
            // Every lane has capacity one. Move the complete compatible wave
            // into permanent workers before awaiting any terminal, so driver
            // cancellation can discard replies but never semantic work.
            match sender.try_send(ReadyCommitWork::new(&self.runtime, assignment, result)) {
                Ok(()) => results.push(receiver),
                Err(error) => {
                    let work = error.into_inner();
                    if let Err(fault) = work.cancel() {
                        cleanup_fault.get_or_insert(fault);
                    }
                    transport_closed = true;
                    break;
                }
            }
        }
        if let Some(fault) = pending.cancel_remaining() {
            cleanup_fault.get_or_insert(fault);
        }
        let mut fault = None;
        for result in results {
            match result.await {
                Ok(AuthorityReadyCommitTerminal::Applied | AuthorityReadyCommitTerminal::Stale) => {
                }
                Ok(AuthorityReadyCommitTerminal::Fault(terminal_fault)) => {
                    fault.get_or_insert(terminal_fault);
                }
                Err(_) => transport_closed = true,
            }
        }
        // An incomplete capability/effect return outranks the work terminal:
        // the latter describes one job, while the former means the remaining
        // wave is not known to have reached a legal terminal.
        if let Some(fault) = cleanup_fault.or(fault) {
            return Err(AuthorityDriverError::Fault(fault));
        }
        if transport_closed {
            return Err(AuthorityDriverError::LifecycleClosed);
        }
        Ok(AuthorityReadyOutcome::Applied)
    }
}

impl ReadyCommitWorker {
    async fn run(mut self) -> Result<(), AuthorityWorkerFault> {
        while let Some(mut work) = self.assignments.recv().await {
            let Some((assignment, result)) = work.take_for_commit() else {
                return Err(AuthorityWorkerFault::authority(
                    AuthorityFault::SchedulerProjection,
                ));
            };
            let terminal = self.runtime.commit_ready_assignment(assignment);
            let _ = result.send(terminal);
        }
        Ok(())
    }
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
        // Build the complete bounded Ready-wave transport before the first
        // authority task starts. Every move-owned job has one permanent lane;
        // the driver owns scheduling only, never post-commit finalization.
        let (ready_waves, ready_commit_workers) = ReadyWaveExecutor::new(self)?;
        let mut tasks = spawn_compute_exchange(
            handle,
            self,
            cache,
            cache_updates,
            command_rx,
            cancel.child_token(),
        )?;
        for worker in ready_commit_workers {
            let role = AuthorityWorkerRole::ReadyCommit(worker.lane.role_id());
            tasks.push(AuthorityWorkerTask {
                role,
                handle: handle.spawn(worker.run()),
            });
        }
        let runtime = self.clone();
        let ready_cancel = cancel.child_token();
        tasks.push(AuthorityWorkerTask {
            role: AuthorityWorkerRole::Ready,
            handle: handle
                .spawn(async move { run_ready_driver(runtime, ready_waves, ready_cancel).await }),
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
    ready_waves: ReadyWaveExecutor,
    cancel: CancellationToken,
) -> Result<(), AuthorityWorkerFault> {
    run_ready_driver_loop(runtime, ready_waves, cancel, || {}).await
}

async fn run_ready_driver_loop(
    runtime: AuthorityRuntime,
    mut ready_waves: ReadyWaveExecutor,
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
        tokio::pin!(work_notified);
        tokio::pin!(capacity_notified);
        let _ = work_notified.as_mut().enable();
        let _ = capacity_notified.as_mut().enable();
        observe_attempt();
        let result = match continuation.take() {
            Some(continuation) => runtime.resume_ready(continuation),
            None => runtime.try_drive_ready(),
        };
        let result = match result {
            Ok(AuthorityReadyDispatch::Outcome(outcome)) => Ok(outcome),
            Ok(AuthorityReadyDispatch::Wave(wave)) => ready_waves.execute(wave).await,
            Err(error) => Err(error),
        };
        let step = match result {
            Ok(AuthorityReadyOutcome::Applied) => WorkerStep::Progress,
            Ok(AuthorityReadyOutcome::Idle) => WorkerStep::WaitForRunnable,
            Ok(AuthorityReadyOutcome::EffectCapacity(blocked)) => {
                continuation = Some(blocked);
                WorkerStep::WaitForEffectCapacity
            }
            Err(error) => classify_driver_error(error)?,
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
                WorkerStep::WaitForRunnable => work_notified.as_mut().await,
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
        (WorkerStep::WaitForEffectCapacity, _) | (_, WorkerStep::WaitForEffectCapacity) => {
            WorkerStep::WaitForEffectCapacity
        }
        (WorkerStep::WaitForRunnable, WorkerStep::WaitForRunnable) => WorkerStep::WaitForRunnable,
    }
}

fn classify_driver_error(error: AuthorityDriverError) -> Result<WorkerStep, AuthorityWorkerFault> {
    match error {
        AuthorityDriverError::Stale => Ok(WorkerStep::Progress),
        AuthorityDriverError::EffectCapacity => Ok(WorkerStep::WaitForEffectCapacity),
        AuthorityDriverError::LifecycleClosed => Err(AuthorityWorkerFault::lifecycle_closed()),
        AuthorityDriverError::Fault(fault) => Err(AuthorityWorkerFault::authority(fault)),
    }
}

#[cfg(test)]
#[path = "tests/support/worker.rs"]
pub(in crate::authority) mod test_support;
