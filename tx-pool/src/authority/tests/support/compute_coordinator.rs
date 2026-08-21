use super::*;
use crate::authority::{
    runtime::{AuthorityCommittedComputeExchange, AuthorityRuntime},
    worker::{AuthorityWorkerFaultKind, AuthorityWorkerSpawnError},
};

/// Drive one already-committed assignment into a deliberately closed stable
/// lane. The production coordinator must return the exact checked-out
/// capability as a structural fault; it may neither retry the transaction as
/// policy pressure nor discard the owner.
pub(in crate::authority) fn closed_assignment_observation(
    runtime: AuthorityRuntime,
    slot: ComputeWorkerSlot,
    committed: AuthorityCommittedComputeExchange,
) -> Result<Option<AuthorityWorkerFaultKind>, AuthorityWorkerSpawnError> {
    let (assignment_tx, assignment_rx) = mpsc::channel(1);
    drop(assignment_rx);
    let (completion_tx, completion_rx) = mpsc::channel(1);
    let (cache_tx, cache_rx) = mpsc::channel(1);
    let (command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
    let mut coordinator = ComputeCoordinator::new(
        runtime,
        vec![CoordinatorLane {
            slot,
            sender: Some(assignment_tx),
            phase: SlotPhase::Idle,
            probe_suppressed: false,
        }],
        completion_rx,
        cache_tx,
        command_rx,
        CancellationToken::new(),
    )?;
    // Keep unrelated channel endpoints alive so only the assignment receiver
    // is the falsified boundary.
    let _endpoints = (completion_tx, cache_rx, command_tx);
    Ok(coordinator
        .consume_exchange(committed)
        .err()
        .map(AuthorityWorkerFault::into_kind))
}

/// Structured owner for an isolated coordinator whose unrelated endpoints
/// remain live while a test closes only the completion ingress.
pub(in crate::authority) struct IsolatedCoordinatorOwner {
    completion: Option<mpsc::Sender<ComputeExchangeCompletion>>,
    _assignments: mpsc::Receiver<AuthorityComputeJob>,
    _cache_updates: mpsc::Receiver<VerificationCacheUpdate>,
    _commands: watch::Sender<ChunkCommand>,
    join: Option<tokio::task::JoinHandle<Result<(), AuthorityWorkerFault>>>,
}

impl IsolatedCoordinatorOwner {
    pub(in crate::authority) fn close_completion_ingress(&mut self) {
        drop(self.completion.take());
    }

    pub(in crate::authority) async fn join(
        mut self,
    ) -> Result<Result<(), AuthorityWorkerFault>, tokio::task::JoinError> {
        self.join
            .take()
            .expect("the isolated coordinator join capability is linear")
            .await
    }
}

impl Drop for IsolatedCoordinatorOwner {
    fn drop(&mut self) {
        if let Some(join) = &self.join {
            join.abort();
        }
    }
}

/// Build a coordinator with no workers while retaining every non-completion
/// endpoint. This makes an abnormal completion close the only exercised
/// lifecycle boundary; the fixture cannot accidentally request shutdown by
/// dropping its command sender.
pub(in crate::authority) fn isolated_coordinator(
    runtime: AuthorityRuntime,
    handle: &Handle,
) -> Result<IsolatedCoordinatorOwner, AuthorityWorkerSpawnError> {
    let (assignment_tx, assignment_rx) = mpsc::channel(1);
    let (completion_tx, completion_rx) = mpsc::channel(1);
    let (cache_tx, cache_rx) = mpsc::channel(1);
    let (command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
    let coordinator = ComputeCoordinator::new(
        runtime,
        vec![CoordinatorLane {
            slot: ComputeWorkerSlot::ordered_resolve(),
            sender: Some(assignment_tx),
            phase: SlotPhase::Idle,
            probe_suppressed: false,
        }],
        completion_rx,
        cache_tx,
        command_rx,
        CancellationToken::new(),
    )?;
    Ok(IsolatedCoordinatorOwner {
        completion: Some(completion_tx),
        _assignments: assignment_rx,
        _cache_updates: cache_rx,
        _commands: command_tx,
        join: Some(handle.spawn(coordinator.run())),
    })
}
