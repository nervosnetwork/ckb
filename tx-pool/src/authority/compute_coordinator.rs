//! Bounded retained-compute exchange topology.
//!
//! The coordinator owns only stable worker-slot transport and move-only
//! compute capabilities. Transaction ownership, scheduling, resource policy
//! and settlement remain inside `TxPoolAuthority::Plan/Apply`.

use super::{
    exchange::{ComputeVerifierSlot, ComputeWorkerGrant, ComputeWorkerSlot},
    plan::{
        AuthorityFault, ComputeExchangeCompletion, ComputePeerExclusion, ComputeSettlementRecovery,
        PlanError,
    },
    resolver::VerificationCacheUpdate,
    resources::ResourceCapacityWaitIdentity,
    runtime::{
        AuthorityCommittedComputeExchange, AuthorityComputeAftermath,
        AuthorityComputeAftermathDisposition, AuthorityComputeAssignment,
        AuthorityComputeExchangeFailure, AuthorityComputeExchangeFollowUp, AuthorityComputeJob,
        AuthorityComputeOutcome, AuthorityPendingSettlement, AuthorityRuntime,
    },
    state::VerifyCapability,
    worker::{
        AuthorityWorkerFault, AuthorityWorkerRole, AuthorityWorkerSpawnError, AuthorityWorkerTask,
    },
};
use crate::constants::MAX_READY_BATCH;
use ckb_async_runtime::Handle;
use ckb_network::PeerIndex;
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

struct CoordinatorLane {
    slot: ComputeWorkerSlot,
    sender: Option<mpsc::Sender<AuthorityComputeJob>>,
    phase: SlotPhase,
    probe_suppressed: bool,
}

#[derive(Debug)]
struct PendingSettlement {
    completion: ComputeExchangeCompletion,
    blame_peer: Option<PeerIndex>,
}

impl PendingSettlement {
    fn new(completion: ComputeExchangeCompletion) -> Self {
        Self {
            completion,
            blame_peer: None,
        }
    }
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
    exact_pending: Vec<PendingSettlement>,
    yield_changed_cut: bool,
    exact_after_effect: Vec<PendingSettlement>,
    resource_contended: bool,
    resource_wait_identity: Option<ResourceCapacityWaitIdentity>,
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

enum CoordinatorDriveError {
    Worker(AuthorityWorkerFault),
}

impl From<AuthorityWorkerFault> for CoordinatorDriveError {
    fn from(error: AuthorityWorkerFault) -> Self {
        Self::Worker(error)
    }
}

type CoordinatorDriveResult<T> = Result<T, CoordinatorDriveError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CoordinatorImmediate {
    Progress,
    Wait,
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
    // `spawn_workers` appends every permanent Ready lane, the Ready driver and
    // maintenance; allowing that append to allocate after these tasks start
    // would make an allocator failure leave a detached partial generation.
    let task_count = slot_count
        .checked_add(MAX_READY_BATCH)
        .and_then(|count| count.checked_add(3))
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
                        self.runtime.execute_verification(request, command_rx).await,
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
        let mut exact_pending = Vec::new();
        let mut exact_after_effect = Vec::new();
        let mut eligible_slots = Vec::new();
        for buffer in [&mut exact_pending, &mut exact_after_effect] {
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
            exact_pending,
            yield_changed_cut: false,
            exact_after_effect,
            resource_contended: false,
            resource_wait_identity: None,
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
            // Subscribe to the current generation's bank before any Plan can
            // observe reservation contention. A terminal between Plan and
            // parking is retained by this enabled waiter; the next loop
            // deliberately clones the possibly replaced generation's bank.
            let resource_identity = signal_runtime.resource_capacity_wait_identity();
            let resource_signal = resource_identity.terminal_signal();
            let resource_notified = resource_signal.notified();
            tokio::pin!(resource_notified);
            let _ = resource_notified.as_mut().enable();
            let compute_notified = signal_runtime.compute_signal().notified();
            let compute_capacity_notified = signal_runtime.compute_capacity_signal().notified();

            self.drain_available_completions()?;
            if self.cancel.is_cancelled() && !self.shutting_down {
                self.begin_shutdown();
            }
            if self.shutting_down && self.is_drained() {
                return Ok(());
            }
            if self.promote_resource_waiters_if_bank_changed(&resource_identity) {
                continue;
            }

            let immediate = match self.drive_immediate() {
                Ok(immediate) => immediate,
                Err(CoordinatorDriveError::Worker(error)) => return Err(error),
            };

            if self.yield_changed_cut {
                // A private changed-cut witness proves that another committed
                // transition invalidated this exact Plan while the owner
                // capability stayed current. Yield once so that transition's
                // wake/publication tail can run, then reclassify from the new
                // cut without depending on a scheduler-head notification.
                self.yield_changed_cut = false;
                tokio::task::yield_now().await;
                self.restart_probe_cycle();
                continue;
            }
            if matches!(immediate, CoordinatorImmediate::Progress) && !self.has_resource_waiters() {
                continue;
            }

            if self.promote_resource_waiters_if_bank_changed(&resource_identity) {
                continue;
            }

            let fair_slot = self.fair_wait_slot();
            let wait_effect = self.has_effect_waiters();
            let wait_resource = self.has_resource_waiters();
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
                _ = compute_notified, if fair_slot.is_none() => {
                    self.restart_probe_cycle();
                },
                _ = compute_capacity_notified, if fair_slot.is_none() => {
                    self.probe_work = true;
                },
                _ = effect_notified.as_mut(), if wait_effect => {
                    self.promote_effect_waiters();
                    self.probe_work = true;
                    effect_notified.set(effect_signal.notified());
                    let _ = effect_notified.as_mut().enable();
                }
                _ = resource_notified.as_mut(), if wait_resource => {
                    self.promote_resource_waiters();
                }
            }
        }
    }

    fn drive_immediate(&mut self) -> CoordinatorDriveResult<CoordinatorImmediate> {
        if !self.exact_pending.is_empty() {
            self.drive_exact()?;
            return Ok(CoordinatorImmediate::Progress);
        }
        if self.should_probe_immediately() {
            let grants = self.collect_immediate_grants(Vec::new())?;
            if !grants.is_empty() {
                self.drive_exchange(grants)?;
                self.drive_exact()?;
                return Ok(CoordinatorImmediate::Progress);
            }
        }
        Ok(CoordinatorImmediate::Wait)
    }

    fn begin_shutdown(&mut self) {
        self.shutting_down = true;
        self.probe_work = false;
        self.seed_grant = None;
        self.yield_changed_cut = false;
        self.resource_contended = false;
        self.resource_wait_identity = None;
        for lane in &mut self.lanes {
            lane.sender = None;
        }
    }

    fn is_drained(&self) -> bool {
        self.seed_grant.is_none()
            && self.exact_pending.is_empty()
            && !self.yield_changed_cut
            && self.exact_after_effect.is_empty()
            && !self.resource_contended
            && self.resource_wait_identity.is_none()
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
        self.exact_pending.push(PendingSettlement::new(completion));
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

    fn is_resumed(&self) -> bool {
        matches!(&*self.command_rx.borrow(), Resume)
    }

    fn has_probeable_slot(&self) -> bool {
        self.lanes.iter().any(|lane| {
            lane.phase == SlotPhase::Idle && lane.sender.is_some() && !lane.probe_suppressed
        })
    }

    fn should_probe_immediately(&self) -> bool {
        !self.shutting_down
            && self.is_resumed()
            && self.probe_work
            && (self.has_probeable_slot() || self.seed_grant.is_some())
    }

    fn fair_wait_slot(&mut self) -> Option<ComputeWorkerSlot> {
        if self.shutting_down || !self.is_resumed() || !self.probe_work || self.seed_grant.is_some()
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
    ) -> CoordinatorDriveResult<Vec<ComputeWorkerGrant>> {
        if self.shutting_down || !self.is_resumed() {
            return Ok(Vec::new());
        }
        let bound = self.lanes.len();
        grants.reserve(bound.saturating_sub(grants.len()));
        if let Some(seed) = self.seed_grant.take() {
            grants.push(seed);
        }
        self.eligible_slots.clear();
        for lane in &mut self.lanes {
            let eligible =
                lane.phase == SlotPhase::Idle && lane.sender.is_some() && !lane.probe_suppressed;
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

    fn drive_exchange(&mut self, grants: Vec<ComputeWorkerGrant>) -> CoordinatorDriveResult<()> {
        let grants = self.collect_immediate_grants(grants)?;
        if grants.is_empty() {
            return Ok(());
        }
        let exclusions = self
            .exact_after_effect
            .iter()
            .filter_map(|pending| {
                pending
                    .blame_peer
                    .map(|peer| ComputePeerExclusion::from_completion(&pending.completion, peer))
            })
            .collect::<Vec<_>>();
        match self.runtime.exchange_compute(grants, &exclusions) {
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
    ) -> CoordinatorDriveResult<()> {
        let AuthorityCommittedComputeExchange {
            capture_failures,
            assignments,
            unused_grants,
            follow_up,
        } = committed;
        drop(unused_grants);
        let pending_fault = match follow_up {
            AuthorityComputeExchangeFollowUp::None => None,
            AuthorityComputeExchangeFollowUp::RetryProbe => {
                self.restart_probe_cycle();
                None
            }
            AuthorityComputeExchangeFollowUp::Fault(fault) => {
                Some(AuthorityWorkerFault::authority(fault))
            }
        };
        for completion in capture_failures {
            let completion = completion.finish_execution();
            self.mark_finished(completion.slot())?;
            self.exact_pending.push(PendingSettlement::new(completion));
        }
        for assignment in assignments {
            let slot = assignment.slot();
            let lane = self.lane_mut(slot)?;
            if lane.phase != SlotPhase::Idle {
                return Err(
                    AuthorityWorkerFault::authority(AuthorityFault::SchedulerProjection).into(),
                );
            }
            let Some(sender) = lane.sender.as_ref() else {
                // Only terminal shutdown or a previously observed closed
                // receiver removes this sender; neither can legally produce a
                // fresh assignment for the lane.
                let completion = assignment.into_requeue_completion();
                lane.phase = SlotPhase::Finished;
                return Err(AuthorityWorkerFault::completion(completion).into());
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
                    return Err(AuthorityWorkerFault::completion(completion).into());
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
                    return Err(AuthorityWorkerFault::completion(completion).into());
                }
            }
        }
        if let Some(fault) = pending_fault {
            return Err(fault.into());
        }
        Ok(())
    }

    fn recover_exchange_failure(
        &mut self,
        failure: AuthorityComputeExchangeFailure,
    ) -> CoordinatorDriveResult<()> {
        let AuthorityComputeExchangeFailure::Plan(failure) = failure;
        match failure.error() {
            PlanError::ResourceContended(identity) => {
                let identity = identity.clone();
                let (_, grants) = failure.into_parts();
                drop(grants);
                self.bind_resource_wait(identity);
                Ok(())
            }
            PlanError::EffectClosed => Err(AuthorityWorkerFault::lifecycle_closed().into()),
            PlanError::Fault(_)
            | PlanError::Stale(_)
            | PlanError::Duplicate
            | PlanError::PayloadVariant
            | PlanError::Membership(_)
            | PlanError::Backpressure(_) => Err(AuthorityWorkerFault::exchange(failure).into()),
        }
    }

    fn drive_exact(&mut self) -> CoordinatorDriveResult<()> {
        self.exact_pending
            .sort_unstable_by_key(|pending| std::cmp::Reverse(pending.completion.version()));
        while let Some(pending) = self.exact_pending.pop() {
            let completion = pending.completion;
            let (slot, finished) = completion.into_parts();
            match self.runtime.settle_finished(finished) {
                ControlFlow::Continue(committed) => {
                    let (aftermath, post_commit_fault) = committed.into_parts();
                    self.mark_idle(slot)?;
                    self.consume_aftermath(aftermath)?;
                    if let Some(fault) = post_commit_fault {
                        return Err(AuthorityWorkerFault::authority(fault).into());
                    }
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
                        ComputeSettlementRecovery::RetryExact(_) => {
                            let settlement = failure.into_settlement();
                            self.exact_pending.push(PendingSettlement::new(
                                ComputeExchangeCompletion::from_finished(
                                    slot,
                                    super::runtime::AuthorityFinishedCompute::from_parts(
                                        settlement, aftermath,
                                    ),
                                ),
                            ));
                            self.yield_changed_cut = true;
                            return Ok(());
                        }
                        ComputeSettlementRecovery::WaitEffectCapacity => {
                            let blame_peer = failure.blame_peer();
                            self.exact_after_effect.push(PendingSettlement {
                                completion: ComputeExchangeCompletion::from_finished(
                                    slot,
                                    super::runtime::AuthorityFinishedCompute::from_parts(
                                        failure.into_settlement(),
                                        aftermath,
                                    ),
                                ),
                                blame_peer,
                            });
                        }
                        ComputeSettlementRecovery::Structural(_) => {
                            return Err(AuthorityWorkerFault::settlement(
                                AuthorityPendingSettlement::from_completion_failure(
                                    failure, aftermath,
                                ),
                            )
                            .into());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn consume_aftermath(
        &mut self,
        aftermath: AuthorityComputeAftermath,
    ) -> Result<(), AuthorityWorkerFault> {
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
            AuthorityComputeAftermathDisposition::Progress => Ok(()),
            AuthorityComputeAftermathDisposition::Fault(fault) => {
                Err(AuthorityWorkerFault::authority(fault))
            }
        }
    }

    fn consume_disposition(
        disposition: AuthorityComputeAftermathDisposition,
    ) -> Result<(), AuthorityWorkerFault> {
        match disposition {
            AuthorityComputeAftermathDisposition::Progress => Ok(()),
            AuthorityComputeAftermathDisposition::Fault(fault) => {
                Err(AuthorityWorkerFault::authority(fault))
            }
        }
    }

    fn has_effect_waiters(&self) -> bool {
        !self.exact_after_effect.is_empty()
    }

    fn promote_effect_waiters(&mut self) {
        self.exact_pending.append(&mut self.exact_after_effect);
    }

    fn has_resource_waiters(&self) -> bool {
        self.resource_contended
    }

    fn bind_resource_wait(&mut self, identity: ResourceCapacityWaitIdentity) {
        match self.resource_wait_identity.as_ref() {
            Some(bound) if bound.same_bank(&identity) => {}
            None => self.resource_wait_identity = Some(identity),
            Some(_) => {
                // Two bank identities cannot share one causal wait. Leaving
                // the identity empty makes the run loop reclassify the whole
                // bounded queue exactly once instead of parking either bank.
                self.resource_wait_identity = None;
            }
        }
        self.resource_contended = true;
    }

    fn resource_wait_matches(&self, current: &ResourceCapacityWaitIdentity) -> bool {
        self.resource_wait_identity
            .as_ref()
            .is_some_and(|bound| bound.same_bank(current))
    }

    fn promote_resource_waiters_if_bank_changed(
        &mut self,
        current: &ResourceCapacityWaitIdentity,
    ) -> bool {
        if !self.has_resource_waiters() || self.resource_wait_matches(current) {
            return false;
        }
        // The parked work belongs to an older generation, or two
        // independently observed banks met before either parked. Generation
        // progress authorizes one reclassification; waiting on either
        // unrelated signal would lose a wake.
        self.promote_resource_waiters();
        true
    }

    fn promote_resource_waiters(&mut self) {
        self.resource_contended = false;
        self.resource_wait_identity = None;
        self.restart_probe_cycle();
    }
}

#[cfg(test)]
#[path = "tests/support/compute_coordinator.rs"]
pub(in crate::authority) mod test_support;
