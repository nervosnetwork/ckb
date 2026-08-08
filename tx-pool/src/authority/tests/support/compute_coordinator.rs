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
            backoff_until: None,
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
