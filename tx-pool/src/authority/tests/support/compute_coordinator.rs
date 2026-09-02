use super::*;
use crate::authority::{
    resources::HeldResourceCapacityReservation,
    runtime::{
        AuthorityCommittedComputeExchange, AuthorityComputeExchangeFollowUp, AuthorityRuntime,
    },
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
        .map(|error| match error {
            CoordinatorDriveError::Worker(error) => error.into_kind(),
            CoordinatorDriveError::ReplaceGeneration => {
                panic!("a closed assignment receiver cannot request generation replacement")
            }
        }))
}

/// Consume the typed result of one stale shared checkout and report whether
/// the exact suppressed lane is immediately eligible for a fresh fair probe.
pub(in crate::authority) fn stale_checkout_reopens_probe(
    runtime: AuthorityRuntime,
    slot: ComputeWorkerSlot,
    grant: ComputeWorkerGrant,
) -> Result<bool, AuthorityWorkerSpawnError> {
    let (assignment_tx, assignment_rx) = mpsc::channel(1);
    let (completion_tx, completion_rx) = mpsc::channel(1);
    let (cache_tx, cache_rx) = mpsc::channel(1);
    let (command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
    let mut coordinator = ComputeCoordinator::new(
        runtime,
        vec![CoordinatorLane {
            slot,
            sender: Some(assignment_tx),
            phase: SlotPhase::Idle,
            probe_suppressed: true,
        }],
        completion_rx,
        cache_tx,
        command_rx,
        CancellationToken::new(),
    )?;
    let _endpoints = (assignment_rx, completion_tx, cache_rx, command_tx);
    assert!(
        coordinator
            .consume_exchange(AuthorityCommittedComputeExchange {
                settled: Vec::new(),
                obsolete: Vec::new(),
                deferred: Vec::new(),
                capture_failures: Vec::new(),
                assignments: Vec::new(),
                unused_grants: vec![grant],
                follow_up: AuthorityComputeExchangeFollowUp::RetryProbe,
            })
            .is_ok(),
        "a stale shared checkout is an ordinary retry level"
    );
    Ok(coordinator.fair_wait_slot() == Some(slot))
}

/// Hold a sibling capacity reservation across one canonical exchange, then
/// prove its bank-owned terminal signal is the only event which reopens the
/// parked completion/probe. The wait owns no execution grant and performs no
/// timer- or loop-driven retry.
pub(in crate::authority) async fn resource_contention_waits_for_bank_terminal(
    runtime: AuthorityRuntime,
    slot: ComputeWorkerSlot,
    grant: ComputeWorkerGrant,
    held: HeldResourceCapacityReservation,
) -> Result<bool, AuthorityWorkerSpawnError> {
    let (assignment_tx, mut assignment_rx) = mpsc::channel(1);
    let (completion_tx, completion_rx) = mpsc::channel(1);
    let (cache_tx, cache_rx) = mpsc::channel(1);
    let (command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
    let mut coordinator = ComputeCoordinator::new(
        runtime.clone(),
        vec![CoordinatorLane {
            slot,
            sender: Some(assignment_tx),
            phase: SlotPhase::Idle,
            // The supplied grant models the seed already acquired by the
            // production select loop.
            probe_suppressed: true,
        }],
        completion_rx,
        cache_tx,
        command_rx,
        CancellationToken::new(),
    )?;
    let _endpoints = (completion_tx, cache_rx, command_tx);

    let signal = runtime.resource_capacity_wait_identity().terminal_signal();
    let notified = signal.notified();
    tokio::pin!(notified);
    let _ = notified.as_mut().enable();

    assert!(
        coordinator.drive_exchange(vec![grant]).is_ok(),
        "capacity reservation competition is an ordinary deferred level"
    );
    assert!(coordinator.has_resource_waiters());
    assert!(matches!(
        assignment_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    held.release();
    notified.as_mut().await;
    coordinator.promote_resource_waiters();
    assert!(!coordinator.has_resource_waiters());

    let reprobe = coordinator
        .fair_wait_slot()
        .expect("the bank terminal reopens the one fair lane");
    let execution = runtime
        .try_acquire_compute_execution()
        .expect("the deferred exchange returned its execution permit");
    assert!(coordinator.mark_probed(reprobe).is_ok());
    coordinator.seed_grant = Some(ComputeWorkerGrant::new(reprobe, execution));
    assert!(matches!(
        coordinator.drive_immediate(),
        Ok(CoordinatorImmediate::Progress)
    ));

    let assignment = assignment_rx
        .try_recv()
        .expect("one signal-gated retry produces one assignment");
    assert!(matches!(
        assignment_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    drop(assignment);
    Ok(true)
}

/// Prove that a wait bound to one generation never parks on a different
/// generation's bank and that the mismatch authorizes exactly one promotion.
pub(in crate::authority) fn resource_wait_bank_change_reclassifies_once(
    runtime: AuthorityRuntime,
    replacement: &AuthorityRuntime,
) -> bool {
    let (assignment_tx, assignment_rx) = mpsc::channel(1);
    let (_completion_tx, completion_rx) = mpsc::channel(1);
    let (cache_tx, cache_rx) = mpsc::channel(1);
    let (_command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
    let mut coordinator = ComputeCoordinator::new(
        runtime.clone(),
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
    )
    .expect("the identity fixture reserves its bounded buffers");
    let _endpoints = (assignment_rx, cache_rx);
    coordinator.probe_work = false;
    coordinator.bind_resource_wait(runtime.resource_capacity_wait_identity());

    let replacement_identity = replacement.resource_capacity_wait_identity();
    assert!(coordinator.promote_resource_waiters_if_bank_changed(&replacement_identity));
    assert!(!coordinator.promote_resource_waiters_if_bank_changed(&replacement_identity));
    !coordinator.has_resource_waiters()
        && coordinator.resource_wait_identity.is_none()
        && coordinator.probe_work
}

/// Exercise the production recovery visitor with one settlement and one fair
/// grant. The resource wait owns the settlement exactly once and owns no
/// execution permit; repeated promotion cannot duplicate either capability.
pub(in crate::authority) fn after_resource_recovery_is_linear(
    runtime: AuthorityRuntime,
    completion: ComputeExchangeCompletion,
    grant: ComputeWorkerGrant,
) -> ComputeExchangeCompletion {
    let completion_slot = completion.slot();
    let grant_slot = grant.slot();
    let (completion_assignment_tx, completion_assignment_rx) = mpsc::channel(1);
    let (grant_assignment_tx, grant_assignment_rx) = mpsc::channel(1);
    let (_completion_tx, completion_rx) = mpsc::channel(1);
    let (cache_tx, cache_rx) = mpsc::channel(1);
    let (_command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
    let mut coordinator = ComputeCoordinator::new(
        runtime.clone(),
        vec![
            CoordinatorLane {
                slot: completion_slot,
                sender: Some(completion_assignment_tx),
                phase: SlotPhase::Idle,
                probe_suppressed: false,
            },
            CoordinatorLane {
                slot: grant_slot,
                sender: Some(grant_assignment_tx),
                phase: SlotPhase::Idle,
                probe_suppressed: false,
            },
        ],
        completion_rx,
        cache_tx,
        command_rx,
        CancellationToken::new(),
    )
    .expect("the recovery fixture reserves its exact bounded queues");
    let _endpoints = (completion_assignment_rx, grant_assignment_rx, cache_rx);

    assert!(runtime.try_acquire_compute_execution().is_none());
    {
        let mut recovery = CoordinatorRecovery {
            coordinator: &mut coordinator,
            route: RecoveryRoute::AfterResource,
        };
        assert!(recovery.recover_settlement(completion).is_ok());
        assert!(recovery.recover_grant(grant).is_ok());
    }
    let returned = runtime
        .try_acquire_compute_execution()
        .expect("resource waiting immediately returns the fair execution permit");
    drop(returned);

    coordinator.bind_resource_wait(runtime.resource_capacity_wait_identity());
    assert_eq!(coordinator.exchange_after_resource.len(), 1);
    coordinator.promote_resource_waiters();
    assert_eq!(coordinator.exchange_after_resource.len(), 0);
    assert_eq!(coordinator.exchange_pending.len(), 1);
    coordinator.promote_resource_waiters();
    assert_eq!(coordinator.exchange_pending.len(), 1);
    let recovered = coordinator
        .exchange_pending
        .pop()
        .expect("one recovered settlement survives promotion");
    assert_eq!(recovered.slot(), completion_slot);
    recovered
}

/// Structured owner for an isolated coordinator whose unrelated endpoints
/// remain live while a test closes only the completion ingress.
pub(in crate::authority) struct IsolatedCoordinatorOwner {
    completion: Option<mpsc::Sender<ComputeExchangeCompletion>>,
    assignments: mpsc::Receiver<AuthorityComputeJob>,
    _cache_updates: mpsc::Receiver<VerificationCacheUpdate>,
    _commands: watch::Sender<ChunkCommand>,
    join: Option<tokio::task::JoinHandle<Result<(), AuthorityWorkerFault>>>,
}

impl IsolatedCoordinatorOwner {
    pub(in crate::authority) async fn next_assignment(&mut self) -> Option<AuthorityComputeJob> {
        self.assignments.recv().await
    }

    pub(in crate::authority) async fn send_completion(
        &self,
        completion: ComputeExchangeCompletion,
    ) -> Result<(), mpsc::error::SendError<ComputeExchangeCompletion>> {
        match &self.completion {
            Some(sender) => sender.send(completion).await,
            None => Err(mpsc::error::SendError(completion)),
        }
    }

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
        assignments: assignment_rx,
        _cache_updates: cache_rx,
        _commands: command_tx,
        join: Some(handle.spawn(coordinator.run())),
    })
}
