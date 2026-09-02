use super::foundation::{genesis_snapshot, runtime_config, tx};
use crate::authority::{
    effect::{
        CommittedEffect, CommittedRejection, EffectBatchBound, EffectBatchBounds, EffectCapacity,
        EffectLimits, EffectPolicy, RejectionAudience,
    },
    exchange::{ComputeVerifierSlot, ComputeWorkerGrant, ComputeWorkerSlot},
    plan::ComputeExchangeCompletion,
    runtime::{AuthorityComputeOutcome, AuthorityRuntime, SettlementOrigin},
    shard::ConcurrentRemovalProbe,
    state::{
        DependencyKey, OwnedTx, PreAcceptedPhase, QueuedWork, ValidatedAdmission, WorkPermit,
        test_support::RejectionKind,
    },
    worker::{
        AuthorityWorkerFaultKind, AuthorityWorkerRole, test_support::AuthorityTestWorkerOwner,
    },
};
use ckb_async_runtime::Handle;
use ckb_network::PeerIndex;
use ckb_script::ChunkCommand;
use ckb_types::{
    core::{FeeRate, TransactionBuilder},
    packed::{Byte32, OutPoint},
};
use std::{ops::ControlFlow, sync::Arc, time::Duration};
use tokio::sync::{RwLock, mpsc};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_compute_coordinator_probes_every_role_with_one_available_fair_permit() {
    let mut config = runtime_config();
    config.max_tx_verify_workers = 1;
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the one-verifier topology is valid");

    let admission = ValidatedAdmission::remote(tx(90_001), PeerIndex::from(1usize))
        .expect("the remote fixture is valid");
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("admission commits");
    let checkout = runtime
        .try_checkout_for_foundation(WorkPermit::ResolveOnly)
        .expect("the manual resolve checkout remains healthy");
    let ControlFlow::Continue(Some(job)) = checkout else {
        panic!("the admitted Resolve owner is ready")
    };
    let AuthorityComputeOutcome::Completion(completion) = runtime.execute_compute(job) else {
        panic!("ResolveOnly cannot continue directly into verification")
    };
    let ControlFlow::Continue(_) = runtime.settle_completion(completion) else {
        panic!("the manual resolution settlement must not block")
    };

    // Leave only one fair permit available. The ordered resolver owns the
    // first fair probe for dependency liveness; after it proves the current
    // Resolve lane empty, the same level must reach the compatible verifier
    // without a new transaction mutation or timer.
    let held = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture retains one Direct-equivalent execution permit");
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let cache = Arc::new(RwLock::new(ckb_verification::cache::init_cache()));
    let (cache_tx, mut cache_rx) = mpsc::channel(4);
    let workers = AuthorityTestWorkerOwner::spawn_set(
        runtime.clone(),
        &handle,
        cache,
        cache_tx,
        ChunkCommand::Resume,
    )
    .expect("the compute exchange topology starts");
    assert_eq!(
        workers.role_count(AuthorityWorkerRole::ComputeCoordinator),
        1
    );

    let _cache_update = tokio::time::timeout(Duration::from_secs(2), cache_rx.recv())
        .await
        .expect("one available fair permit must reach the compatible verifier role")
        .expect("a cache miss publishes its update after settlement");
    assert!(runtime.with_authority_for_foundation(|authority| {
        matches!(
            authority.entry(&key),
            Some(OwnedTx::Accepted(_))
                | Some(OwnedTx::PreAccepted(
                    crate::authority::state::PreAcceptedEntry {
                        phase: PreAcceptedPhase::Ready(_),
                        ..
                    }
                ))
        )
    }));

    workers
        .shutdown()
        .await
        .expect("the structured worker generation closes cleanly");
    drop(held);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_compute_coordinator_retries_exact_after_changed_dependency_cut_without_notify() {
    const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

    let mut config = runtime_config();
    config.max_tx_verify_workers = 1;
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the one-lane runtime is valid");
    let admission = ValidatedAdmission::remote(tx(90_101), PeerIndex::from(101usize))
        .expect("the Remote fixture is valid");
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("the Remote owner commits");

    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let mut coordinator =
        crate::authority::compute_coordinator::test_support::isolated_coordinator(
            runtime.clone(),
            &handle,
        )
        .expect("the isolated production coordinator starts");
    let job = tokio::time::timeout(PROBE_TIMEOUT, coordinator.next_assignment())
        .await
        .expect("the coordinator checks out the Resolve owner")
        .expect("the stable assignment channel remains open");
    let dependency = DependencyKey::Cell(OutPoint::new(Byte32::new([0xa1; 32]), 0));
    let completion = job
        .missing_for_foundation(vec![dependency.clone()])
        .into_exchange_completion_for_foundation(ComputeWorkerSlot::ordered_resolve());
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_read_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_compute_settlement_commit_probe(Some(probe));
    });
    coordinator
        .send_completion(completion)
        .await
        .expect("the exact completion enters its assigned lane");
    entered
        .recv_timeout(PROBE_TIMEOUT)
        .expect("the first exact Plan reaches its final-cut interposer");
    runtime.with_authority_read_for_foundation(|authority| {
        authority
            .apply_dependency_loss_during_shared_plan_for_foundation(vec![dependency])
            .expect("the real dependency frontier advances without a compute-head notify");
        authority
            .entries_for_reference()
            .set_compute_settlement_commit_probe(None);
    });
    release
        .send(())
        .expect("the stale first exact Plan resumes");
    let retry = tokio::time::timeout(PROBE_TIMEOUT, coordinator.next_assignment())
        .await
        .expect("the changed-cut yield automatically probes the authoritative Resolve level")
        .expect("the isolated assignment transport remains live");
    drop(coordinator);
    assert!(matches!(
        runtime.settle_compute(retry.retry_for_foundation(), SettlementOrigin::Completion),
        ControlFlow::Continue(())
    ));
    runtime.with_authority_read_for_foundation(|authority| {
        assert!(matches!(
            authority.entry(&key),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
        ));
        assert_eq!(authority.resources().preaccepted().active_work, 0);
    });
}

#[test]
fn uak_closed_assignment_transport_returns_the_exact_checked_out_capability() {
    let mut config = runtime_config();
    config.max_tx_verify_workers = 1;
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the one-verifier topology is valid");
    let admission = ValidatedAdmission::remote(tx(90_005), PeerIndex::from(3usize))
        .expect("the transport fixture is valid");
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("admission commits");
    let slot = ComputeWorkerSlot::ordered_resolve();
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture owns one exact fair execution permit");
    let committed = match runtime
        .exchange_compute(Vec::new(), vec![ComputeWorkerGrant::new(slot, execution)])
    {
        Ok(committed) => committed,
        Err(failure) => {
            drop(failure);
            panic!("the initial exchange checks out the resolver owner")
        }
    };

    let observation =
        crate::authority::compute_coordinator::test_support::closed_assignment_observation(
            runtime.clone(),
            slot,
            committed,
        )
        .expect("the test coordinator reserves its bounded buffers")
        .expect("a closed assignment receiver is a structural transport fault");
    let AuthorityWorkerFaultKind::Completion(completion) = observation else {
        panic!("the transport fault must retain the exact completion capability")
    };
    assert_eq!(completion.slot(), slot);
    let (_, finished) = (*completion).into_parts();
    assert!(matches!(
        runtime.settle_finished(finished),
        ControlFlow::Continue(_)
    ));
    assert!(runtime.with_authority_for_foundation(|authority| {
        matches!(
            authority.entry(&key),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
        )
    }));
}

#[test]
fn uak_stale_shared_checkout_reopens_the_suppressed_fair_probe() {
    let mut config = runtime_config();
    config.max_tx_verify_workers = 1;
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the one-verifier topology is valid");
    let slot = ComputeWorkerSlot::ordered_resolve();
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture owns one exact fair execution permit");
    assert!(
        crate::authority::compute_coordinator::test_support::stale_checkout_reopens_probe(
            runtime,
            slot,
            ComputeWorkerGrant::new(slot, execution),
        )
        .expect("the isolated coordinator reserves its bounded buffers")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_resource_reservation_contention_waits_for_the_bank_terminal() {
    let mut config = runtime_config();
    config.max_tx_verify_workers = 1;
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the one-verifier topology is valid");
    runtime
        .admit(
            ValidatedAdmission::remote(tx(90_006), PeerIndex::from(4usize))
                .expect("the retained fixture is valid"),
        )
        .expect("the queued owner commits");
    let held = runtime
        .with_authority_for_foundation(|authority| {
            authority.hold_positive_compute_reservation_for_foundation()
        })
        .expect("the sibling plan holds the exact positive capacity");
    let slot = ComputeWorkerSlot::ordered_resolve();
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture owns one fair execution permit");

    let completed = tokio::time::timeout(
        Duration::from_secs(2),
        crate::authority::compute_coordinator::test_support::resource_contention_waits_for_bank_terminal(
            runtime,
            slot,
            ComputeWorkerGrant::new(slot, execution),
            held,
        ),
    )
    .await
    .expect("a reservation terminal cannot strand the deferred exchange")
    .expect("the isolated coordinator reserves its bounded buffers");
    assert!(completed);
}

#[test]
fn uak_resource_wait_reclassifies_once_when_the_bank_identity_changes() {
    let mut config = runtime_config();
    config.max_tx_verify_workers = 1;
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the original generation is valid");
    let replacement = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the replacement generation is valid");

    assert!(
        crate::authority::compute_coordinator::test_support::resource_wait_bank_change_reclassifies_once(
            runtime,
            &replacement,
        )
    );
}

#[test]
fn uak_after_resource_recovery_preserves_one_completion_and_returns_the_grant() {
    let mut config = runtime_config();
    config.max_tx_verify_workers = 1;
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the recovery generation is valid");
    runtime
        .admit(
            ValidatedAdmission::remote(tx(90_007), PeerIndex::from(5usize))
                .expect("the recovery fixture is valid"),
        )
        .expect("the queued owner commits");
    let checkout = runtime
        .try_checkout_for_foundation(WorkPermit::ResolveOnly)
        .expect("the recovery fixture remains healthy");
    let ControlFlow::Continue(Some(job)) = checkout else {
        panic!("the queued owner yields one resolve job")
    };
    let AuthorityComputeOutcome::Completion(completion) = runtime.execute_compute(job) else {
        panic!("ResolveOnly produces a settlement rather than inline verification")
    };
    let completion = ComputeExchangeCompletion::from_finished(
        ComputeWorkerSlot::ordered_resolve(),
        completion.finish_execution(),
    );

    let grant_execution = runtime
        .try_compute_execution_for_foundation()
        .expect("one fair permit becomes the recovered grant");
    let mut held = Vec::new();
    while let Some(permit) = runtime.try_compute_execution_for_foundation() {
        held.push(permit);
    }
    let grant_slot = ComputeWorkerSlot::from(ComputeVerifierSlot::new(
        0,
        crate::authority::state::VerifyCapability::Any,
    ));
    let recovered =
        crate::authority::compute_coordinator::test_support::after_resource_recovery_is_linear(
            runtime.clone(),
            completion,
            ComputeWorkerGrant::new(grant_slot, grant_execution),
        );
    drop(held);

    let (slot, finished) = recovered.into_parts();
    assert_eq!(slot, ComputeWorkerSlot::ordered_resolve());
    assert!(matches!(
        runtime.settle_finished(finished),
        ControlFlow::Continue(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_effect_blocked_completion_observes_a_later_fair_permit_release() {
    const EFFECT_BYTES: usize = 1024 * 1024;
    let mut config = runtime_config();
    config.max_tx_verify_workers = 1;
    config.min_fee_rate = FeeRate::from_u64(1_000);
    let snapshot = genesis_snapshot();
    let effects = EffectLimits::partitioned(
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectBatchBounds::new(
            EffectBatchBound::new(1, EFFECT_BYTES),
            EffectBatchBound::new(1, EFFECT_BYTES),
            EffectBatchBound::new(1, EFFECT_BYTES),
        ),
    )
    .expect("the fixture admits one effect per region");
    let runtime = AuthorityRuntime::new_with_effect_limits_for_foundation(
        &config,
        snapshot.consensus(),
        Arc::clone(&snapshot),
        effects,
    )
    .expect("the narrow effect runtime is valid");
    runtime
        .queue_effect_for_foundation(
            EffectPolicy::Remote,
            CommittedEffect::Rejected(CommittedRejection::for_foundation(
                Arc::new(TransactionBuilder::default().version(90_002u32).build()),
                RejectionAudience::foundation(),
                RejectionKind::Policy,
            )),
        )
        .expect("the occupied Remote effect commits");

    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let cache = Arc::new(RwLock::new(ckb_verification::cache::init_cache()));
    let (cache_tx, _cache_rx) = mpsc::channel(4);
    let workers = AuthorityTestWorkerOwner::spawn_set(
        runtime.clone(),
        &handle,
        cache,
        cache_tx,
        ChunkCommand::Resume,
    )
    .expect("the compute exchange topology starts");

    let blocked = ValidatedAdmission::remote(tx(90_003), PeerIndex::from(2usize))
        .expect("the blocked rejection fixture is valid");
    let blocked_key = blocked.identity.raw.clone();
    runtime.admit(blocked).expect("the first admission commits");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let checked_out = runtime.with_authority_for_foundation(|authority| {
                matches!(
                    authority.entry(&blocked_key),
                    Some(OwnedTx::PreAccepted(entry))
                        if matches!(entry.phase, PreAcceptedPhase::Computing(_))
                )
            });
            if checked_out {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the effect-blocked owner is checked out");

    // Freeze new checkout before observing the released-permit level. Without
    // this cut, the coordinator may fairly reacquire a permit between the
    // availability read and the fixture's two try-acquires, making the test a
    // scheduler race rather than a wake-protocol falsifier.
    workers
        .send(ChunkCommand::Suspend)
        .expect("the fixture suspends new verification checkout");
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime.available_compute_permits_for_foundation() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the suspended effect-blocked completion releases its fair execution permit");

    let first_held = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture holds the first fair permit");
    let second_held = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture holds the second fair permit");
    let queued = ValidatedAdmission::proposal(tx(90_004)).expect("the Proposal fixture is valid");
    let queued_key = queued.identity.raw.clone();
    runtime.admit(queued).expect("the second admission commits");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(runtime.with_authority_for_foundation(|authority| {
        matches!(
            authority.entry(&queued_key),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
        )
    }));

    // The coordinator cannot join the fair wait queue while it owns the first
    // finished capability. Releasing a Direct-equivalent permit must still
    // publish a level that triggers an immediate acquisition and bounded role
    // probe for the unrelated owner.
    workers
        .send(ChunkCommand::Resume)
        .expect("the fixture resumes the coordinator before publishing capacity");
    drop(first_held);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let progressed = runtime.with_authority_for_foundation(|authority| {
                !matches!(
                    authority.entry(&queued_key),
                    Some(OwnedTx::PreAccepted(entry))
                        if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
                )
            });
            if progressed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the RAII fair-capacity level prevents a lost retained wake");
    drop(second_held);

    let occupied = runtime
        .wait_effect_publication_for_foundation()
        .await
        .expect("the occupied effect remains publishable");
    runtime
        .settle_effect_for_foundation(occupied.complete_for_foundation().published())
        .expect("effect capacity is released through its authoritative boundary");
    for _ in 0..1 {
        let rejection = tokio::time::timeout(
            Duration::from_secs(2),
            runtime.wait_effect_publication_for_foundation(),
        )
        .await
        .expect("each blocked rejection reaches the effect log")
        .expect("the effect log remains open");
        runtime
            .settle_effect_for_foundation(rejection.complete_for_foundation().published())
            .expect("each rejection publication releases the next waiter");
    }
    workers
        .shutdown()
        .await
        .expect("the structured worker generation closes cleanly");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_shutdown_drains_effect_blocked_completion_after_ingress_closes() {
    const EFFECT_BYTES: usize = 1024 * 1024;
    let mut config = runtime_config();
    config.max_tx_verify_workers = 1;
    config.min_fee_rate = FeeRate::from_u64(1_000);
    let snapshot = genesis_snapshot();
    let effects = EffectLimits::partitioned(
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectBatchBounds::new(
            EffectBatchBound::new(1, EFFECT_BYTES),
            EffectBatchBound::new(1, EFFECT_BYTES),
            EffectBatchBound::new(1, EFFECT_BYTES),
        ),
    )
    .expect("the fixture admits one effect per region");
    let runtime = AuthorityRuntime::new_with_effect_limits_for_foundation(
        &config,
        snapshot.consensus(),
        Arc::clone(&snapshot),
        effects,
    )
    .expect("the narrow effect runtime is valid");
    runtime
        .queue_effect_for_foundation(
            EffectPolicy::Remote,
            CommittedEffect::Rejected(CommittedRejection::for_foundation(
                Arc::new(TransactionBuilder::default().version(90_011u32).build()),
                RejectionAudience::foundation(),
                RejectionKind::Policy,
            )),
        )
        .expect("the occupied Remote effect commits");

    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let cache = Arc::new(RwLock::new(ckb_verification::cache::init_cache()));
    let (cache_tx, _cache_rx) = mpsc::channel(4);
    let workers = AuthorityTestWorkerOwner::spawn_set(
        runtime.clone(),
        &handle,
        cache,
        cache_tx,
        ChunkCommand::Resume,
    )
    .expect("the compute exchange topology starts");

    let blocked = ValidatedAdmission::remote(tx(90_012), PeerIndex::from(4usize))
        .expect("the effect-blocked fixture is valid");
    let blocked_key = blocked.identity.raw.clone();
    runtime.admit(blocked).expect("the admission commits");
    tokio::time::timeout(Duration::from_secs(2), async {
        while !runtime.with_authority_for_foundation(|authority| {
            matches!(
                authority.entry(&blocked_key),
                Some(OwnedTx::PreAccepted(entry))
                    if matches!(entry.phase, PreAcceptedPhase::Computing(_))
            )
        }) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the effect-blocked owner is checked out");

    workers
        .send(ChunkCommand::Suspend)
        .expect("the fixture suspends new checkout");
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime.available_compute_permits_for_foundation() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the finished capability parks behind effect capacity");

    // Shutdown joins the workers before the coordinator. Therefore all
    // completion senders retire while the finished capability is still
    // effect-blocked. The old biased receive arm then returned `None`
    // forever and could not observe the later capacity release.
    let shutdown = tokio::spawn(workers.shutdown());
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
    let occupied = runtime
        .wait_effect_publication_for_foundation()
        .await
        .expect("the occupied effect remains publishable");
    runtime
        .settle_effect_for_foundation(occupied.complete_for_foundation().published())
        .expect("effect capacity is released through its authority boundary");

    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime.with_authority_for_foundation(|authority| {
            matches!(
                authority.entry(&blocked_key),
                Some(OwnedTx::PreAccepted(entry))
                    if matches!(entry.phase, PreAcceptedPhase::Computing(_))
            )
        }) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("closed completion ingress cannot starve the effect-drain rank");
    shutdown
        .await
        .expect("the shutdown owner task joins")
        .expect("the structured worker generation closes cleanly");
    assert_eq!(runtime.available_compute_permits_for_foundation(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_completion_ingress_close_outside_shutdown_is_a_lifecycle_fault() {
    let mut config = runtime_config();
    config.max_tx_verify_workers = 1;
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the isolated coordinator runtime is valid");
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let mut owner =
        crate::authority::compute_coordinator::test_support::isolated_coordinator(runtime, &handle)
            .expect("the isolated coordinator starts");

    owner.close_completion_ingress();
    let fault = tokio::time::timeout(Duration::from_secs(2), owner.join())
        .await
        .expect("the abnormal ingress close is observed")
        .expect("the coordinator task joins")
        .expect_err("an ingress close outside shutdown is not a clean drain");
    assert!(matches!(
        fault.into_kind(),
        AuthorityWorkerFaultKind::LifecycleClosed
    ));
}
