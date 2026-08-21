//! Bounded retained-compute exchange topology.
//!
//! The coordinator owns only stable worker-slot transport and move-only
//! compute capabilities. Transaction ownership, scheduling, resource policy
//! and settlement remain inside `TxPoolAuthority::Plan/Apply`.

use super::{
    exchange::{ComputeVerifierSlot, ComputeWorkerGrant, ComputeWorkerSlot},
    plan::{
        AuthorityFault, Backpressure, ComputeCancellationError, ComputeExchangeCompletion,
        ComputeExchangeDeferredRoute, ComputeExchangeRecoveries, ComputeExchangeRecoverySink,
        ComputeSettlementRecovery, PlanError,
    },
    resolver::VerificationCacheUpdate,
    runtime::{
        AuthorityCommittedComputeExchange, AuthorityComputeAftermath,
        AuthorityComputeAftermathDisposition, AuthorityComputeAssignment,
        AuthorityComputeExchangeFailure, AuthorityComputeJob, AuthorityComputeOutcome,
        AuthorityGenerationReplacementError, AuthorityPendingSettlement, AuthorityRuntime,
    },
    state::VerifyCapability,
    worker::{
        AuthorityWorkerFault, AuthorityWorkerRole, AuthorityWorkerSpawnError, AuthorityWorkerTask,
    },
};
use ckb_async_runtime::Handle;
use ckb_script::{ChunkCommand, ChunkCommand::Resume};
use ckb_stop_handler::CancellationToken;
use ckb_verification::cache::TxVerificationCache;
use std::{ops::ControlFlow, sync::Arc};
use tokio::sync::{RwLock, mpsc, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotPhase {
    Idle,
    Assigned,
    Finished,
}

#[derive(Clone, Copy)]
enum RecoveryRoute {
    Exact,
    AfterEffect,
}

struct CoordinatorLane {
    slot: ComputeWorkerSlot,
    sender: Option<mpsc::Sender<AuthorityComputeJob>>,
    phase: SlotPhase,
    probe_suppressed: bool,
}

enum RetainedWorkerRole {
    OrderedResolve {
        slot: ComputeWorkerSlot,
    },
    Verifier {
        slot: ComputeVerifierSlot,
        cache: Arc<RwLock<TxVerificationCache>>,
    },
}

impl RetainedWorkerRole {
    fn slot(&self) -> ComputeWorkerSlot {
        match self {
            Self::OrderedResolve { slot } => *slot,
            Self::Verifier { slot, .. } => (*slot).into(),
        }
    }
}

struct RetainedComputeWorker {
    runtime: AuthorityRuntime,
    role: RetainedWorkerRole,
    assignments: mpsc::Receiver<AuthorityComputeJob>,
    completions: mpsc::Sender<ComputeExchangeCompletion>,
}

struct WorkerSpawnSpec {
    role: AuthorityWorkerRole,
    worker: RetainedComputeWorker,
}

struct ComputeCoordinator {
    runtime: AuthorityRuntime,
    lanes: Vec<CoordinatorLane>,
    completions: mpsc::Receiver<ComputeExchangeCompletion>,
    cache_updates: mpsc::Sender<VerificationCacheUpdate>,
    command_rx: watch::Receiver<ChunkCommand>,
    cancel: CancellationToken,
    exchange_pending: Vec<ComputeExchangeCompletion>,
    exact_pending: Vec<ComputeExchangeCompletion>,
    exchange_after_effect: Vec<ComputeExchangeCompletion>,
    exact_after_effect: Vec<ComputeExchangeCompletion>,
    eligible_slots: Vec<ComputeWorkerSlot>,
    seed_grant: Option<ComputeWorkerGrant>,
    probe_work: bool,
    shutting_down: bool,
    completion_ingress: CompletionIngress,
}

/// Lifecycle of the stable worker-completion input. Closure is absorbing:
/// after every worker sender has exited, `recv()` can only return `None` and
/// must be retired from the biased wait instead of competing forever with
/// effect-capacity progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionIngress {
    Open,
    Closed,
}

struct CoordinatorRecovery<'coordinator> {
    coordinator: &'coordinator mut ComputeCoordinator,
    route: RecoveryRoute,
}

impl ComputeExchangeRecoverySink for CoordinatorRecovery<'_> {
    type Error = AuthorityWorkerFault;

    fn recover_settlement(
        &mut self,
        completion: ComputeExchangeCompletion,
    ) -> Result<(), Self::Error> {
        match self.route {
            RecoveryRoute::Exact => self.coordinator.exact_pending.push(completion),
            RecoveryRoute::AfterEffect => {
                self.coordinator.exchange_after_effect.push(completion);
            }
        }
        Ok(())
    }

    fn recover_obsolete(&mut self, slot: ComputeWorkerSlot) -> Result<(), Self::Error> {
        self.coordinator.mark_idle(slot)
    }

    fn recover_grant(&mut self, grant: ComputeWorkerGrant) -> Result<(), Self::Error> {
        drop(grant);
        Ok(())
    }
}

pub(in crate::authority) fn spawn_compute_exchange(
    handle: &Handle,
    runtime: &AuthorityRuntime,
    cache: Arc<RwLock<TxVerificationCache>>,
    cache_updates: mpsc::Sender<VerificationCacheUpdate>,
    command_rx: watch::Receiver<ChunkCommand>,
    cancel: CancellationToken,
) -> Result<Vec<AuthorityWorkerTask>, AuthorityWorkerSpawnError> {
    let verifier_count = runtime.verify_worker_count();
    let slot_count = verifier_count
        .checked_add(1)
        .ok_or(AuthorityWorkerSpawnError::Allocation)?;
    // Reserve the complete authority-worker vector before the first spawn.
    // The two non-compute drivers are appended by `spawn_workers`; allowing
    // that append to allocate after these tasks start would make an allocator
    // failure leave a detached partial generation.
    let task_count = slot_count
        .checked_add(3)
        .ok_or(AuthorityWorkerSpawnError::Allocation)?;
    let mut tasks = Vec::new();
    tasks
        .try_reserve(task_count)
        .map_err(|_| AuthorityWorkerSpawnError::Allocation)?;
    let mut lanes = Vec::new();
    lanes
        .try_reserve(slot_count)
        .map_err(|_| AuthorityWorkerSpawnError::Allocation)?;
    let mut worker_specs = Vec::new();
    worker_specs
        .try_reserve(slot_count)
        .map_err(|_| AuthorityWorkerSpawnError::Allocation)?;
    let (completion_tx, completion_rx) = mpsc::channel(slot_count);

    let resolver_slot = ComputeWorkerSlot::ordered_resolve();
    let (resolver_tx, resolver_rx) = mpsc::channel(1);
    lanes.push(CoordinatorLane {
        slot: resolver_slot,
        sender: Some(resolver_tx),
        phase: SlotPhase::Idle,
        probe_suppressed: false,
    });
    worker_specs.push(WorkerSpawnSpec {
        role: AuthorityWorkerRole::Resolver,
        worker: RetainedComputeWorker {
            runtime: runtime.clone(),
            role: RetainedWorkerRole::OrderedResolve {
                slot: resolver_slot,
            },
            assignments: resolver_rx,
            completions: completion_tx.clone(),
        },
    });

    for worker_id in 0..verifier_count {
        let capability = if worker_id == 0 && verifier_count > 1 {
            VerifyCapability::SmallCycleOnly
        } else {
            VerifyCapability::Any
        };
        let slot = ComputeVerifierSlot::new(worker_id, capability);
        let worker_slot = ComputeWorkerSlot::from(slot);
        let (assignment_tx, assignment_rx) = mpsc::channel(1);
        lanes.push(CoordinatorLane {
            slot: worker_slot,
            sender: Some(assignment_tx),
            phase: SlotPhase::Idle,
            probe_suppressed: false,
        });
        worker_specs.push(WorkerSpawnSpec {
            role: AuthorityWorkerRole::Verifier(slot.worker_id()),
            worker: RetainedComputeWorker {
                runtime: runtime.clone(),
                role: RetainedWorkerRole::Verifier {
                    slot,
                    cache: Arc::clone(&cache),
                },
                assignments: assignment_rx,
                completions: completion_tx.clone(),
            },
        });
    }
    drop(completion_tx);

    let coordinator = ComputeCoordinator::new(
        runtime.clone(),
        lanes,
        completion_rx,
        cache_updates,
        command_rx.clone(),
        cancel.child_token(),
    )?;
    // The topology joins from the end of this vector. Keeping the coordinator
    // first makes it the last joined compute task, after every worker has
    // returned its final completion and observed its assignment channel close.
    tasks.push(AuthorityWorkerTask {
        role: AuthorityWorkerRole::ComputeCoordinator,
        handle: handle.spawn(coordinator.run()),
    });
    for spec in worker_specs {
        tasks.push(AuthorityWorkerTask {
            role: spec.role,
            handle: handle.spawn(spec.worker.run(command_rx.clone())),
        });
    }
    Ok(tasks)
}

impl RetainedComputeWorker {
    async fn run(
        mut self,
        mut command_rx: watch::Receiver<ChunkCommand>,
    ) -> Result<(), AuthorityWorkerFault> {
        while let Some(job) = self.assignments.recv().await {
            let (completion, fault) = self.execute(job, &mut command_rx).await;
            let completion = ComputeExchangeCompletion::from_finished(
                self.role.slot(),
                completion.finish_execution(),
            );
            if let Err(error) = self.completions.send(completion).await {
                return Err(AuthorityWorkerFault::completion(error.0));
            }
            if let Some(fault) = fault {
                return Err(AuthorityWorkerFault::authority(fault));
            }
        }
        Ok(())
    }

    async fn execute(
        &self,
        job: AuthorityComputeJob,
        command_rx: &mut watch::Receiver<ChunkCommand>,
    ) -> (
        super::runtime::AuthorityComputeCompletion,
        Option<AuthorityFault>,
    ) {
        match self.runtime.execute_compute(job) {
            AuthorityComputeOutcome::Completion(completion) => (completion, None),
            AuthorityComputeOutcome::Verification(request) => match &self.role {
                RetainedWorkerRole::Verifier { cache, .. } => {
                    let request = {
                        let guard = cache.read().await;
                        request.bind_cache(&guard)
                    };
                    (
                        self.runtime
                            .execute_verification(request, Some(command_rx))
                            .await,
                        None,
                    )
                }
                RetainedWorkerRole::OrderedResolve { .. } => {
                    (request.retry(), Some(AuthorityFault::SchedulerProjection))
                }
            },
        }
    }
}

impl ComputeCoordinator {
    fn new(
        runtime: AuthorityRuntime,
        lanes: Vec<CoordinatorLane>,
        completions: mpsc::Receiver<ComputeExchangeCompletion>,
        cache_updates: mpsc::Sender<VerificationCacheUpdate>,
        command_rx: watch::Receiver<ChunkCommand>,
        cancel: CancellationToken,
    ) -> Result<Self, AuthorityWorkerSpawnError> {
        let bound = lanes.len();
        let mut exchange_pending = Vec::new();
        let mut exact_pending = Vec::new();
        let mut exchange_after_effect = Vec::new();
        let mut exact_after_effect = Vec::new();
        let mut eligible_slots = Vec::new();
        for buffer in [
            &mut exchange_pending,
            &mut exact_pending,
            &mut exchange_after_effect,
            &mut exact_after_effect,
        ] {
            buffer
                .try_reserve(bound)
                .map_err(|_| AuthorityWorkerSpawnError::Allocation)?;
        }
        eligible_slots
            .try_reserve(bound)
            .map_err(|_| AuthorityWorkerSpawnError::Allocation)?;
        Ok(Self {
            runtime,
            lanes,
            completions,
            cache_updates,
            command_rx,
            cancel,
            exchange_pending,
            exact_pending,
            exchange_after_effect,
            exact_after_effect,
            eligible_slots,
            seed_grant: None,
            probe_work: true,
            shutting_down: false,
            completion_ingress: CompletionIngress::Open,
        })
    }

    async fn run(mut self) -> Result<(), AuthorityWorkerFault> {
        let signal_runtime = self.runtime.clone();
        let effect_signal = signal_runtime.effect_capacity_signal();
        let effect_notified = effect_signal.notified();
        tokio::pin!(effect_notified);
        let _ = effect_notified.as_mut().enable();
        loop {
            let compute_notified = signal_runtime.compute_signal().notified();
            let compute_capacity_notified = signal_runtime.compute_capacity_signal().notified();

            self.drain_available_completions()?;
            if self.cancel.is_cancelled() && !self.shutting_down {
                self.begin_shutdown();
            }
            if self.shutting_down && self.is_drained() {
                return Ok(());
            }

            if !self.exchange_pending.is_empty() {
                self.drive_exchange(Vec::new())?;
                self.drive_exact()?;
                continue;
            }
            if !self.exact_pending.is_empty() {
                self.drive_exact()?;
                continue;
            }
            if self.should_probe_immediately() {
                let grants = self.collect_immediate_grants(Vec::new())?;
                if !grants.is_empty() {
                    self.drive_exchange(grants)?;
                    self.drive_exact()?;
                    continue;
                }
            }

            let fair_slot = self.fair_wait_slot();
            let wait_effect = self.has_effect_waiters();
            let cancel = self.cancel.clone();
            let runtime = self.runtime.clone();
            let completion = &mut self.completions;
            let command_rx = &mut self.command_rx;
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if !self.shutting_down => {
                    self.begin_shutdown();
                }
                received = completion.recv(), if self.completion_ingress == CompletionIngress::Open => {
                    match received {
                        Some(completion) => self.accept_completion(completion)?,
                        None if self.shutting_down => {
                            self.completion_ingress = CompletionIngress::Closed;
                        }
                        None => return Err(AuthorityWorkerFault::lifecycle_closed()),
                    }
                }
                changed = command_rx.changed() => {
                    if changed.is_err() {
                        self.begin_shutdown();
                    } else if matches!(&*command_rx.borrow(), Resume) {
                        self.restart_probe_cycle();
                    } else {
                        self.seed_grant = None;
                    }
                }
                permit = runtime.acquire_compute_execution(&cancel), if fair_slot.is_some() => {
                    if let (Some(slot), Some(permit)) = (fair_slot, permit) {
                        self.mark_probed(slot)?;
                        self.seed_grant = Some(ComputeWorkerGrant::new(slot, permit));
                    }
                }
                _ = compute_notified, if fair_slot.is_none() => self.restart_probe_cycle(),
                _ = compute_capacity_notified, if fair_slot.is_none() => {
                    self.probe_work = true;
                },
                _ = effect_notified.as_mut(), if wait_effect => {
                    self.promote_effect_waiters();
                    self.probe_work = true;
                    effect_notified.set(effect_signal.notified());
                    let _ = effect_notified.as_mut().enable();
                }
            }
        }
    }

    fn begin_shutdown(&mut self) {
        self.shutting_down = true;
        self.probe_work = false;
        self.seed_grant = None;
        for lane in &mut self.lanes {
            lane.sender = None;
        }
    }

    fn is_drained(&self) -> bool {
        self.seed_grant.is_none()
            && self.exchange_pending.is_empty()
            && self.exact_pending.is_empty()
            && self.exchange_after_effect.is_empty()
            && self.exact_after_effect.is_empty()
            && self.lanes.iter().all(|lane| lane.phase == SlotPhase::Idle)
    }

    fn drain_available_completions(&mut self) -> Result<(), AuthorityWorkerFault> {
        loop {
            match self.completions.try_recv() {
                Ok(completion) => self.accept_completion(completion)?,
                Err(mpsc::error::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::error::TryRecvError::Disconnected) if self.shutting_down => {
                    self.completion_ingress = CompletionIngress::Closed;
                    return Ok(());
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return Err(AuthorityWorkerFault::lifecycle_closed());
                }
            }
        }
    }

    fn accept_completion(
        &mut self,
        completion: ComputeExchangeCompletion,
    ) -> Result<(), AuthorityWorkerFault> {
        let lane = self.lane_mut(completion.slot())?;
        if lane.phase != SlotPhase::Assigned {
            return Err(AuthorityWorkerFault::completion(completion));
        }
        lane.phase = SlotPhase::Finished;
        lane.probe_suppressed = false;
        self.exchange_pending.push(completion);
        self.probe_work = true;
        Ok(())
    }

    fn lane_mut(
        &mut self,
        slot: ComputeWorkerSlot,
    ) -> Result<&mut CoordinatorLane, AuthorityWorkerFault> {
        self.lanes
            .iter_mut()
            .find(|lane| lane.slot.id() == slot.id())
            .ok_or_else(|| AuthorityWorkerFault::authority(AuthorityFault::SchedulerProjection))
    }

    fn mark_idle(&mut self, slot: ComputeWorkerSlot) -> Result<(), AuthorityWorkerFault> {
        let lane = self.lane_mut(slot)?;
        if lane.phase != SlotPhase::Finished {
            return Err(AuthorityWorkerFault::authority(
                AuthorityFault::SchedulerProjection,
            ));
        }
        lane.phase = SlotPhase::Idle;
        lane.probe_suppressed = false;
        Ok(())
    }

    fn mark_finished(&mut self, slot: ComputeWorkerSlot) -> Result<(), AuthorityWorkerFault> {
        let lane = self.lane_mut(slot)?;
        if lane.phase != SlotPhase::Idle {
            return Err(AuthorityWorkerFault::authority(
                AuthorityFault::SchedulerProjection,
            ));
        }
        lane.phase = SlotPhase::Finished;
        lane.probe_suppressed = false;
        Ok(())
    }

    fn has_finished(&self) -> bool {
        self.lanes
            .iter()
            .any(|lane| lane.phase == SlotPhase::Finished)
    }

    fn is_resumed(&self) -> bool {
        matches!(&*self.command_rx.borrow(), Resume)
    }

    fn should_probe_immediately(&self) -> bool {
        !self.shutting_down
            && self.is_resumed()
            && self.probe_work
            && (self.has_finished() || self.seed_grant.is_some())
    }

    fn fair_wait_slot(&mut self) -> Option<ComputeWorkerSlot> {
        if self.shutting_down
            || !self.is_resumed()
            || !self.probe_work
            || self.has_finished()
            || self.seed_grant.is_some()
        {
            return None;
        }
        self.lanes
            .iter_mut()
            .find(|lane| {
                lane.phase == SlotPhase::Idle && lane.sender.is_some() && !lane.probe_suppressed
            })
            .map(|lane| lane.slot)
    }

    fn collect_immediate_grants(
        &mut self,
        mut grants: Vec<ComputeWorkerGrant>,
    ) -> Result<Vec<ComputeWorkerGrant>, AuthorityWorkerFault> {
        if self.shutting_down || !self.is_resumed() {
            return Ok(Vec::new());
        }
        let bound = self.lanes.len();
        if grants
            .try_reserve(bound.saturating_sub(grants.len()))
            .is_err()
        {
            self.seed_grant = None;
            drop(grants);
            self.replace_generation_after_allocation()?;
            return Ok(Vec::new());
        }
        if let Some(seed) = self.seed_grant.take() {
            grants.push(seed);
        }
        self.eligible_slots.clear();
        for lane in &mut self.lanes {
            let eligible = match lane.phase {
                SlotPhase::Idle => lane.sender.is_some() && !lane.probe_suppressed,
                SlotPhase::Assigned => false,
                SlotPhase::Finished => {
                    lane.sender.is_some()
                        && !lane.probe_suppressed
                        && self.exchange_pending.iter().any(|completion| {
                            completion.slot().id() == lane.slot.id()
                                && completion.permits_immediate_refill()
                        })
                }
            };
            if eligible {
                self.eligible_slots.push(lane.slot);
            }
        }
        self.eligible_slots
            .sort_unstable_by_key(|slot| std::cmp::Reverse(slot.canonical_key()));
        while let Some(slot) = self.eligible_slots.pop() {
            if grants.iter().any(|grant| grant.slot().id() == slot.id()) {
                continue;
            }
            let Some(execution) = self.runtime.try_acquire_compute_execution() else {
                break;
            };
            self.mark_probed(slot)?;
            grants.push(ComputeWorkerGrant::new(slot, execution));
        }
        Ok(grants)
    }

    fn mark_probed(&mut self, slot: ComputeWorkerSlot) -> Result<(), AuthorityWorkerFault> {
        let lane = self.lane_mut(slot)?;
        lane.probe_suppressed = true;
        Ok(())
    }

    fn restart_probe_cycle(&mut self) {
        for lane in &mut self.lanes {
            lane.probe_suppressed = false;
        }
        self.probe_work = true;
    }

    fn drive_exchange(
        &mut self,
        grants: Vec<ComputeWorkerGrant>,
    ) -> Result<(), AuthorityWorkerFault> {
        let grants = self.collect_immediate_grants(grants)?;
        if self.exchange_pending.is_empty() && grants.is_empty() {
            return Ok(());
        }
        let mut replacement = Vec::new();
        if replacement.try_reserve(self.lanes.len()).is_err() {
            drop(grants);
            self.replace_generation_after_allocation()?;
            return Ok(());
        }
        let completions = std::mem::replace(&mut self.exchange_pending, replacement);
        match self.runtime.exchange_compute(completions, grants) {
            Ok(committed) => {
                self.consume_exchange(committed)?;
            }
            Err(failure) => self.recover_exchange_failure(failure)?,
        }
        Ok(())
    }

    fn consume_exchange(
        &mut self,
        committed: AuthorityCommittedComputeExchange,
    ) -> Result<(), AuthorityWorkerFault> {
        let AuthorityCommittedComputeExchange {
            settled,
            obsolete,
            deferred,
            capture_failures,
            assignments,
            unused_grants,
        } = committed;
        let made_progress = !settled.is_empty()
            || !obsolete.is_empty()
            || !capture_failures.is_empty()
            || !assignments.is_empty();
        drop(unused_grants);
        let mut pending_fault = None;
        let mut replace_generation = false;
        for settled in settled {
            let (slot, aftermath) = settled.into_parts();
            self.mark_idle(slot)?;
            match self.consume_aftermath(aftermath) {
                Ok(replace) => replace_generation |= replace,
                Err(fault) => pending_fault = Some(fault),
            }
        }
        for slot in obsolete {
            self.mark_idle(slot)?;
        }
        for deferred in deferred {
            let (route, completion) = deferred.into_parts();
            match route {
                ComputeExchangeDeferredRoute::ExactSettlement => {
                    self.exact_pending.push(completion)
                }
                ComputeExchangeDeferredRoute::ExchangeRetry => {
                    self.exchange_pending.push(completion)
                }
                ComputeExchangeDeferredRoute::ExchangeAfterEffect => {
                    self.exchange_after_effect.push(completion)
                }
            }
        }
        for completion in capture_failures {
            let completion = completion.finish_execution();
            self.mark_finished(completion.slot())?;
            self.exchange_pending.push(completion);
        }
        for assignment in assignments {
            let slot = assignment.slot();
            let lane = self.lane_mut(slot)?;
            if lane.phase != SlotPhase::Idle {
                return Err(AuthorityWorkerFault::authority(
                    AuthorityFault::SchedulerProjection,
                ));
            }
            if replace_generation {
                let completion = assignment.into_requeue_completion();
                lane.phase = SlotPhase::Finished;
                self.exact_pending.push(completion);
                continue;
            }
            let Some(sender) = lane.sender.as_ref() else {
                // Only terminal shutdown or a previously observed closed
                // receiver removes this sender; neither can legally produce a
                // fresh assignment for the lane.
                let completion = assignment.into_requeue_completion();
                lane.phase = SlotPhase::Finished;
                return Err(AuthorityWorkerFault::completion(completion));
            };
            let (_, job) = assignment.into_parts();
            match sender.try_send(job) {
                Ok(()) => lane.phase = SlotPhase::Assigned,
                Err(mpsc::error::TrySendError::Full(job)) => {
                    // The coordinator is the sole sender, and a lane becomes
                    // Idle only after its worker dequeued the prior job and
                    // returned the matching completion. Full therefore proves
                    // transport projection drift, never transaction pressure.
                    let assignment = AuthorityComputeAssignment::from_parts(slot, job);
                    let completion = assignment.into_requeue_completion();
                    lane.phase = SlotPhase::Finished;
                    return Err(AuthorityWorkerFault::completion(completion));
                }
                Err(mpsc::error::TrySendError::Closed(job)) => {
                    // A closed retained receiver proves its owned worker has
                    // terminated. Preserve the checked-out owner in the fault;
                    // supervision will invalidate this generation rather than
                    // silently running with a smaller verifier topology.
                    let assignment = AuthorityComputeAssignment::from_parts(slot, job);
                    let completion = assignment.into_requeue_completion();
                    lane.sender = None;
                    lane.phase = SlotPhase::Finished;
                    return Err(AuthorityWorkerFault::completion(completion));
                }
            }
        }
        if let Some(fault) = pending_fault {
            return Err(fault);
        }
        if replace_generation {
            self.replace_generation_after_allocation()?;
        }
        if made_progress {
            self.restart_probe_cycle();
        }
        Ok(())
    }

    fn recover_exchange_failure(
        &mut self,
        failure: AuthorityComputeExchangeFailure,
    ) -> Result<(), AuthorityWorkerFault> {
        match failure {
            AuthorityComputeExchangeFailure::Allocation {
                completions,
                grants,
            } => {
                drop(grants);
                self.exact_pending.extend(completions);
                self.replace_generation_after_allocation()?;
                Ok(())
            }
            AuthorityComputeExchangeFailure::Plan(failure) => match failure.error() {
                PlanError::Backpressure(Backpressure::Allocation) => {
                    let (_, recoveries) = failure.into_recovery();
                    let result = self.recover_plan_capabilities(recoveries, RecoveryRoute::Exact);
                    result?;
                    self.replace_generation_after_allocation()
                }
                PlanError::Backpressure(Backpressure::EffectCapacity) => {
                    let (_, recoveries) = failure.into_recovery();
                    self.recover_plan_capabilities(recoveries, RecoveryRoute::AfterEffect)
                }
                PlanError::EffectClosed => Err(AuthorityWorkerFault::lifecycle_closed()),
                PlanError::Fault(_)
                | PlanError::Stale(_)
                | PlanError::Duplicate
                | PlanError::PayloadVariant
                | PlanError::Membership(_)
                | PlanError::Backpressure(_) => Err(AuthorityWorkerFault::exchange(failure)),
            },
        }
    }

    fn recover_plan_capabilities(
        &mut self,
        recoveries: ComputeExchangeRecoveries,
        route: RecoveryRoute,
    ) -> Result<(), AuthorityWorkerFault> {
        recoveries.recover_into(&mut CoordinatorRecovery {
            coordinator: self,
            route,
        })
    }

    fn drive_exact(&mut self) -> Result<(), AuthorityWorkerFault> {
        let mut replace_generation = false;
        self.exact_pending
            .sort_unstable_by_key(|completion| std::cmp::Reverse(completion.version()));
        while let Some(completion) = self.exact_pending.pop() {
            let (slot, finished) = completion.into_parts();
            match self.runtime.settle_finished(finished) {
                ControlFlow::Continue(aftermath) => {
                    self.mark_idle(slot)?;
                    replace_generation |= self.consume_aftermath(aftermath)?;
                    self.probe_work = true;
                }
                ControlFlow::Break(pending) => {
                    let (failure, aftermath) = pending.into_parts();
                    match failure.recovery() {
                        ComputeSettlementRecovery::Obsolete(_) => {
                            drop(failure);
                            self.mark_idle(slot)?;
                            self.consume_obsolete_origin(aftermath)?;
                        }
                        ComputeSettlementRecovery::CancelAfterAllocation => {
                            let cancellation = failure.discard_result_for_cancellation();
                            let result = self.runtime.cancel_compute_after_allocation(cancellation);
                            self.mark_idle(slot)?;
                            replace_generation |= self.consume_cancellation(result, aftermath)?;
                        }
                        ComputeSettlementRecovery::WaitEffectCapacity => {
                            self.exact_after_effect
                                .push(ComputeExchangeCompletion::from_finished(
                                    slot,
                                    super::runtime::AuthorityFinishedCompute::from_parts(
                                        failure.into_settlement(),
                                        aftermath,
                                    ),
                                ));
                        }
                        ComputeSettlementRecovery::Structural(_) => {
                            return Err(AuthorityWorkerFault::settlement(
                                AuthorityPendingSettlement::from_completion_failure(
                                    failure, aftermath,
                                ),
                            ));
                        }
                    }
                }
            }
        }
        if replace_generation {
            self.replace_generation_after_allocation()?;
        }
        Ok(())
    }

    fn consume_aftermath(
        &mut self,
        aftermath: AuthorityComputeAftermath,
    ) -> Result<bool, AuthorityWorkerFault> {
        let disposition = aftermath.disposition();
        let (_, cache_update) = aftermath.into_parts();
        if let Some(update) = cache_update {
            let _ = self.cache_updates.try_send(update);
        }
        Self::consume_disposition(disposition)
    }

    fn consume_obsolete_origin(
        &self,
        aftermath: AuthorityComputeAftermath,
    ) -> Result<(), AuthorityWorkerFault> {
        match aftermath.disposition() {
            AuthorityComputeAftermathDisposition::Progress
            | AuthorityComputeAftermathDisposition::ReplaceGeneration => Ok(()),
            AuthorityComputeAftermathDisposition::Fault(fault) => {
                Err(AuthorityWorkerFault::authority(fault))
            }
        }
    }

    fn consume_cancellation(
        &self,
        result: Result<(), ComputeCancellationError>,
        aftermath: AuthorityComputeAftermath,
    ) -> Result<bool, AuthorityWorkerFault> {
        match result {
            Ok(()) => match aftermath.disposition() {
                AuthorityComputeAftermathDisposition::Progress
                | AuthorityComputeAftermathDisposition::ReplaceGeneration => Ok(true),
                AuthorityComputeAftermathDisposition::Fault(fault) => {
                    Err(AuthorityWorkerFault::authority(fault))
                }
            },
            Err(ComputeCancellationError::Obsolete(_)) => {
                self.consume_obsolete_origin(aftermath)?;
                Ok(false)
            }
            Err(ComputeCancellationError::Fault(fault)) => {
                Err(AuthorityWorkerFault::authority(fault))
            }
            Err(ComputeCancellationError::EffectClosed) => {
                Err(AuthorityWorkerFault::lifecycle_closed())
            }
        }
    }

    fn consume_disposition(
        disposition: AuthorityComputeAftermathDisposition,
    ) -> Result<bool, AuthorityWorkerFault> {
        match disposition {
            AuthorityComputeAftermathDisposition::Progress => Ok(false),
            AuthorityComputeAftermathDisposition::ReplaceGeneration => Ok(true),
            AuthorityComputeAftermathDisposition::Fault(fault) => {
                Err(AuthorityWorkerFault::authority(fault))
            }
        }
    }

    fn replace_generation_after_allocation(&mut self) -> Result<(), AuthorityWorkerFault> {
        self.seed_grant = None;
        let total_finished = self
            .exact_pending
            .len()
            .checked_add(self.exchange_pending.len())
            .and_then(|count| count.checked_add(self.exchange_after_effect.len()))
            .and_then(|count| count.checked_add(self.exact_after_effect.len()))
            .ok_or_else(|| AuthorityWorkerFault::authority(AuthorityFault::CounterExhausted))?;
        if total_finished > self.lanes.len() || total_finished > self.exact_pending.capacity() {
            return Err(AuthorityWorkerFault::authority(
                AuthorityFault::SchedulerProjection,
            ));
        }
        self.runtime
            .replace_current_generation_after_allocation()
            .map_err(map_generation_replacement_error)?;
        self.exact_pending.append(&mut self.exchange_pending);
        self.exact_pending.append(&mut self.exchange_after_effect);
        self.exact_pending.append(&mut self.exact_after_effect);
        self.restart_probe_cycle();
        Ok(())
    }

    fn has_effect_waiters(&self) -> bool {
        !self.exchange_after_effect.is_empty() || !self.exact_after_effect.is_empty()
    }

    fn promote_effect_waiters(&mut self) {
        self.exchange_pending
            .append(&mut self.exchange_after_effect);
        self.exact_pending.append(&mut self.exact_after_effect);
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
#[path = "tests/support/compute_coordinator.rs"]
pub(in crate::authority) mod test_support;
