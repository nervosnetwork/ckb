use super::{
    AuthorityAdministrationError, AuthorityComputeCheckout, AuthorityComputeError,
    AuthorityComputeJob, AuthorityComputeOutcome, AuthorityComputeSettlement,
    AuthorityDirectAdmissionError, AuthorityDirectRejectionExecution,
    AuthorityDirectResolutionOutcome, AuthorityDriverError, AuthorityReadyOutcome,
    AuthorityRuntime, AuthorityRuntimeConfig, FinalAdmissionCaptureError, PREACCEPTED_ENTRY_BYTES,
    PlanError, ReadyValidationError, RuntimeConfigError, SettlementOrigin,
    runtime_authority_config_error, runtime_resource_config_error,
    test_support::FoundationCheckoutError,
};
use crate::authority::effect::{
    CommittedEffect, CommittedRejection, EffectBatchBound, EffectBatchBounds, EffectCapacity,
    EffectConfigError, EffectLimits, EffectPolicy, RejectionAudience,
};
use crate::authority::plan::{AuthorityConfigError, AuthorityFault, Backpressure, StalePlan};
use crate::authority::resources::ResourceConfigError;
use crate::authority::state::{
    ChainRevision, ChainViewId, OwnedTx, PreAcceptedPhase, QueuedWork, RemoteResidencyLease,
    ValidatedAdmission, VerifyCapability, WorkPermit, test_support::RejectionKind,
};
use crate::authority::validation::FinalAdmissionValidationError;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_async_runtime::Handle;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_network::PeerIndex;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::CancellationToken;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    U256,
    core::{FeeRate, TransactionBuilder},
    prelude::Unpack,
};
use std::ops::ControlFlow;
use std::sync::Arc;
use tokio::sync::{RwLock as TokioRwLock, mpsc, watch};

fn runtime_config() -> TxPoolConfig {
    TxPoolConfig {
        max_tx_pool_size: 180_000_000,
        max_tx_pool_resident_size: 1_000_000_000,
        min_fee_rate: FeeRate::zero(),
        min_rbf_rate: FeeRate::zero(),
        max_tx_verify_cycles: 70_000_000,
        max_tx_verify_workers: 4,
        max_ancestors_count: 125,
        keep_rejected_tx_hashes_days: 1,
        keep_rejected_tx_hashes_count: 1_000,
        persisted_data: Default::default(),
        recent_reject: Default::default(),
        expiry_hours: 24,
        verify_ordering: VerifyOrdering::ArrivalTime,
        max_tx_pipeline_resident_size: 384_000_000,
    }
}

fn genesis_snapshot() -> Arc<Snapshot> {
    let consensus = Arc::new(ConsensusBuilder::default().build());
    let store = MockStore::default();
    let genesis = consensus.genesis_block();
    Arc::new(Snapshot::new(
        genesis.header(),
        U256::zero(),
        consensus.genesis_epoch_ext().clone(),
        store.store().get_snapshot(),
        Default::default(),
        consensus,
    ))
}

fn runtime() -> AuthorityRuntime {
    let snapshot = genesis_snapshot();
    AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
        .expect("the production authority runtime fixture is valid")
}

fn runtime_with_effect_limits(
    config: &TxPoolConfig,
    snapshot: Arc<Snapshot>,
    effects: EffectLimits,
) -> AuthorityRuntime {
    AuthorityRuntime::new_with_effect_limits_for_foundation(
        config,
        snapshot.consensus(),
        Arc::clone(&snapshot),
        effects,
    )
    .expect("the narrow effect runtime reserves every bounded projection")
}

fn queue_remote_rejection(runtime: &AuthorityRuntime, nonce: u32) {
    let publication = {
        let store = runtime.store.read();
        store
            .authority
            .effect_publication_for_foundation(
                EffectPolicy::Remote,
                vec![CommittedEffect::Rejected(
                    CommittedRejection::for_foundation(
                        Arc::new(TransactionBuilder::default().version(nonce).build()),
                        RejectionAudience::foundation(),
                        RejectionKind::Policy,
                    ),
                )],
            )
            .expect("the runtime effect fixture is bounded")
    };
    let retirement = {
        let mut store = runtime.store.write();
        store
            .authority
            .plan_effect_publication_for_foundation(&publication)
            .expect("the runtime effect fixture fits its region")
            .apply()
    };
    runtime.publish_committed(retirement);
}

fn admission(nonce: u32, peer: usize) -> ValidatedAdmission {
    ValidatedAdmission::remote(
        TransactionBuilder::default().version(nonce).build(),
        PeerIndex::from(peer),
    )
    .expect("the runtime fixture has valid ingress evidence")
}

fn admission_with_cycles(nonce: u32, peer: usize, declared_cycles: u64) -> ValidatedAdmission {
    ValidatedAdmission::remote_with_lease(
        TransactionBuilder::default().version(nonce).build(),
        RemoteResidencyLease::for_foundation(PeerIndex::from(peer)),
        declared_cycles,
    )
    .expect("the runtime fixture has valid ingress evidence")
}

async fn expect_signal(signal: &tokio::sync::Notify, message: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(50), signal.notified())
        .await
        .expect(message);
}

async fn expect_no_signal(signal: &tokio::sync::Notify, message: &str) {
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), signal.notified())
            .await
            .is_err(),
        "{message}"
    );
}

fn retry(job: AuthorityComputeJob) -> AuthorityComputeSettlement {
    job.retry_for_foundation()
}

fn is_queued_resolve(runtime: &AuthorityRuntime, key: &super::super::state::RawTxHash) -> bool {
    let store = runtime.store.read();
    matches!(
        store.authority.entry(key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    )
}

fn continued<T>(flow: ControlFlow<super::AuthorityPendingSettlement, T>) -> T {
    match flow {
        ControlFlow::Continue(value) => value,
        ControlFlow::Break(_) => panic!("the fixture has sufficient effect capacity"),
    }
}

#[test]
fn runtime_checkout_observes_preexisting_level_without_a_wake_hint() {
    let runtime = runtime();
    let admission = admission(901, 91);
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("admission commits");

    let job = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveOnly)
            .expect("checkout remains healthy"),
    )
    .expect("queued work is an authoritative level");
    assert!(matches!(
        runtime.settle_compute(retry(job), SettlementOrigin::Completion),
        ControlFlow::Continue(())
    ));
    assert!(is_queued_resolve(&runtime, &key));
}

#[test]
fn runtime_stale_plan_disposition_depends_on_the_producer_boundary() {
    assert!(matches!(
        AuthorityDriverError::from_ready_plan(PlanError::Stale(StalePlan::Version)),
        AuthorityDriverError::Stale
    ));
    assert!(matches!(
        AuthorityDriverError::from_maintenance_plan(PlanError::Stale(StalePlan::Version)),
        AuthorityDriverError::Fault(AuthorityFault::MembershipProjection)
    ));
    assert!(matches!(
        AuthorityComputeError::from_checkout_plan(PlanError::Stale(StalePlan::Version)),
        AuthorityComputeError::Fault(AuthorityFault::SchedulerProjection)
    ));
    assert!(matches!(
        AuthorityDriverError::from_initial_ready_capture(FinalAdmissionCaptureError::Plan(
            PlanError::Stale(StalePlan::Version),
        )),
        AuthorityDriverError::Fault(AuthorityFault::SchedulerProjection)
    ));
    assert!(matches!(
        AuthorityDriverError::from_ready_preparation(FinalAdmissionCaptureError::Validation(
            FinalAdmissionValidationError::StaleView,
        ),),
        AuthorityDriverError::Fault(AuthorityFault::MembershipProjection)
    ));
    assert!(matches!(
        AuthorityDriverError::from_ready_recheck(FinalAdmissionCaptureError::Plan(
            PlanError::Stale(StalePlan::Version),
        )),
        AuthorityDriverError::Stale
    ));
    assert!(matches!(
        AuthorityDriverError::from_ready_validation(ReadyValidationError::Candidate(
            FinalAdmissionValidationError::StaleView,
        )),
        AuthorityDriverError::Fault(AuthorityFault::MembershipProjection)
    ));
    assert_eq!(
        AuthorityDirectAdmissionError::from_validation(FinalAdmissionValidationError::StaleView,),
        AuthorityDirectAdmissionError::Stale
    );
    assert_eq!(
        AuthorityDirectAdmissionError::from_plan(PlanError::Backpressure(
            Backpressure::ProposalCollision,
        )),
        AuthorityDirectAdmissionError::ProposalCollision
    );
    assert_eq!(
        AuthorityDirectAdmissionError::from_plan(PlanError::Backpressure(
            Backpressure::EffectCapacity,
        )),
        AuthorityDirectAdmissionError::EffectCapacity
    );
    assert_eq!(
        AuthorityAdministrationError::from_plan(PlanError::Backpressure(Backpressure::Allocation,)),
        AuthorityAdministrationError::Allocation
    );
    assert_eq!(
        AuthorityAdministrationError::from_plan(PlanError::Backpressure(
            Backpressure::EffectCapacity,
        )),
        AuthorityAdministrationError::EffectCapacity
    );
    assert_eq!(
        AuthorityAdministrationError::from_plan(PlanError::Stale(StalePlan::Version)),
        AuthorityAdministrationError::Fault(AuthorityFault::MembershipProjection)
    );
}

#[test]
fn runtime_resolution_uses_assembled_policy_and_settles_before_returning() {
    let runtime = runtime();
    let admission = admission(904, 94);
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("admission commits");
    let job = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveOnly)
            .expect("checkout remains healthy"),
    )
    .expect("resolve work is ready");
    assert!(matches!(
        continued(
            runtime
                .execute_compute(job)
                .expect("the assembled zero-fee policy accepts this fixture")
        ),
        AuthorityComputeOutcome::Settled
    ));
    let store = runtime.store.read();
    assert!(matches!(
        store.authority.entry(&key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
    ));
}

#[tokio::test]
async fn runtime_compute_wakes_route_each_head_to_one_compatible_waiter_class() {
    let runtime = runtime();

    runtime
        .admit(admission(1_064, 99))
        .expect("the small-cycle admission commits");
    expect_signal(
        runtime.resolve_signal(),
        "admission must publish the shared Resolve level",
    )
    .await;
    expect_no_signal(
        runtime.verify_small_signal(),
        "Resolve admission must not publish a Verify hint",
    )
    .await;
    expect_no_signal(
        runtime.verify_any_signal(),
        "Resolve admission must not duplicate a Verify hint",
    )
    .await;

    let small = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveOnly)
            .expect("small-cycle resolution checkout remains healthy"),
    )
    .expect("small-cycle resolution is ready");
    assert!(matches!(
        continued(
            runtime
                .execute_compute(small)
                .expect("small-cycle resolution settles")
        ),
        AuthorityComputeOutcome::Settled
    ));
    expect_signal(
        runtime.verify_small_signal(),
        "a shared Small/Any head must publish exactly the Small signal",
    )
    .await;
    expect_no_signal(
        runtime.verify_any_signal(),
        "a shared Small/Any head must not publish a duplicate Any signal",
    )
    .await;

    let large_cycles = runtime_config()
        .max_tx_verify_cycles
        .checked_add(1)
        .expect("the fixture cycle declaration is bounded");
    runtime
        .admit(admission_with_cycles(1_065, 1, large_cycles))
        .expect("the large-cycle admission commits");
    expect_signal(
        runtime.resolve_signal(),
        "the second admission must republish the Resolve baton",
    )
    .await;
    let large = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveOnly)
            .expect("large-cycle resolution checkout remains healthy"),
    )
    .expect("large-cycle resolution is ready");
    assert!(matches!(
        continued(
            runtime
                .execute_compute(large)
                .expect("large-cycle resolution settles")
        ),
        AuthorityComputeOutcome::Settled
    ));
    expect_signal(
        runtime.verify_any_signal(),
        "a distinct large head must publish the Any-only signal",
    )
    .await;
    expect_signal(
        runtime.verify_small_signal(),
        "releasing an active-work slot must republish the unchanged Small head",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_continuous_worker_and_ready_driver_close_one_owner_lifecycle() {
    let runtime = runtime();
    let admission = admission(905, 95);
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("admission commits");
    let job = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveThenVerify(VerifyCapability::Any))
            .expect("checkout remains healthy"),
    )
    .expect("resolve work is ready");
    let AuthorityComputeOutcome::Verification(request) = continued(
        runtime
            .execute_compute(job)
            .expect("resolution continues under the same worker capability"),
    ) else {
        panic!("the empty-script fixture fits continuous verification")
    };
    let cache = ckb_verification::cache::init_cache();
    let verification = continued(
        runtime
            .execute_verification(request.bind_cache(&cache), None)
            .await
            .expect("verification settles Ready ownership"),
    );
    assert!(verification.cache_update.is_some());
    assert_eq!(
        runtime
            .try_drive_ready()
            .expect("the sealed Ready batch commits"),
        AuthorityReadyOutcome::Applied
    );
    let store = runtime.store.read();
    assert!(matches!(
        store.authority.entry(&key),
        Some(OwnedTx::Accepted(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_shared_compute_gate_bounds_mixed_retained_and_direct_work() {
    let runtime = runtime();
    runtime
        .admit(admission(906, 96))
        .expect("the retained fixture enters the authority");
    let retained_execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the retained worker acquires one shared slot");
    let retained = match runtime
        .try_checkout(WorkPermit::ResolveOnly, retained_execution)
        .expect("retained checkout remains healthy")
    {
        ControlFlow::Continue(AuthorityComputeCheckout::Job(job)) => job,
        ControlFlow::Continue(AuthorityComputeCheckout::Idle(execution)) => {
            drop(execution);
            panic!("the retained fixture has queued resolve work")
        }
        ControlFlow::Break(_) => panic!("the fixture has sufficient effect capacity"),
    };

    let direct_execution = runtime
        .try_compute_execution_for_foundation()
        .expect("direct work shares the same partition");
    let direct_tx = TransactionBuilder::default().version(1u32).build();
    let AuthorityDirectResolutionOutcome::Rejected(direct) = runtime
        .resolve_test_accept_transaction(&direct_tx, direct_execution)
        .expect("the direct fixture reaches a stable typed rejection")
    else {
        panic!("the non-zero version fixture must reject before verification")
    };

    let remaining = runtime.available_compute_permits_for_foundation();
    let mut holders = Vec::new();
    holders
        .try_reserve(remaining)
        .expect("the bounded test holder vector allocates");
    for _ in 0..remaining {
        holders.push(
            runtime
                .try_compute_execution_for_foundation()
                .expect("every remaining configured slot is obtainable exactly once"),
        );
    }
    assert_eq!(runtime.available_compute_permits_for_foundation(), 0);
    assert!(runtime.try_compute_execution_for_foundation().is_none());
    {
        let store = runtime.store.read();
        assert_eq!(store.authority.resources().preaccepted().active_work, 1);
    }

    let waiter = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            let cancel = CancellationToken::new();
            runtime.acquire_compute_execution(&cancel).await
        })
    };
    tokio::task::yield_now().await;
    assert!(
        !waiter.is_finished(),
        "the next execution cannot start while retained and direct work saturate the gate"
    );
    let released = holders
        .pop()
        .expect("the fixture retained one spare holder");
    drop(released);
    let replacement = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("one released slot wakes exactly one waiter")
        .expect("the compute waiter task remains healthy")
        .expect("the compute waiter was not cancelled");

    assert!(matches!(
        runtime.settle_compute(
            retained.retry_for_foundation(),
            SettlementOrigin::Completion
        ),
        ControlFlow::Continue(())
    ));
    assert!(matches!(
        runtime
            .settle_direct_transaction_rejection(direct)
            .expect("the direct TestAccept rejection settles read-only"),
        AuthorityDirectRejectionExecution::TestAccept(_)
    ));
    drop(holders);
    drop(replacement);
    assert_eq!(
        runtime.available_compute_permits_for_foundation(),
        runtime.verify_worker_count() + 1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_sealed_worker_set_honors_pause_and_closes_the_owner_lifecycle() {
    let runtime = runtime();
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let cache = Arc::new(TokioRwLock::new(ckb_verification::cache::init_cache()));
    let (cache_tx, mut cache_rx) = mpsc::channel(4);
    let (command_tx, command_rx) = watch::channel(ChunkCommand::Suspend);
    let cancel = CancellationToken::new();
    let handles = runtime
        .spawn_workers(&handle, cache, cache_tx, command_rx, cancel.clone())
        .expect("the validated worker topology reserves its handle vector");
    assert_eq!(
        handles
            .tasks
            .iter()
            .filter(|task| matches!(
                task.role,
                crate::authority::worker::AuthorityWorkerRole::Verifier(_)
            ))
            .count(),
        4
    );

    let admission = admission(906, 96);
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("admission commits");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(is_queued_resolve(&runtime, &key));
    assert_eq!(
        runtime
            .store
            .read()
            .authority
            .resources()
            .preaccepted()
            .active_work,
        0,
        "a suspended topology must not check out a linear capability"
    );

    command_tx
        .send(ChunkCommand::Resume)
        .expect("the worker command authority remains live");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if matches!(
                runtime.store.read().authority.entry(&key),
                Some(OwnedTx::Accepted(_))
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the sealed workers converge the transaction to Accepted");
    let update = tokio::time::timeout(std::time::Duration::from_secs(1), cache_rx.recv())
        .await
        .expect("the best-effort cache effect is not delayed")
        .expect("the cache receiver remains open");
    let expected_witness: [u8; 32] = TransactionBuilder::default()
        .version(906u32)
        .build()
        .witness_hash()
        .unpack();
    assert_eq!(update.key.witness_hash(), &expected_witness);

    cancel.cancel();
    for task in handles.tasks {
        task.handle
            .await
            .expect("authority worker task remains healthy")
            .expect("authority worker exits without a structural fault");
    }
    assert_eq!(
        runtime
            .store
            .read()
            .authority
            .resources()
            .preaccepted()
            .active_work,
        0,
        "structured cancellation cannot strand checked-out work"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_role_batons_drain_a_coalesced_preexisting_frontier() {
    const TRANSACTIONS: usize = 64;

    let runtime = runtime();
    let mut keys = Vec::new();
    keys.try_reserve(TRANSACTIONS)
        .expect("the bounded fixture reserves its key list");
    for index in 0..TRANSACTIONS {
        let nonce = 1_000u32
            .checked_add(u32::try_from(index).expect("the fixture count fits u32"))
            .expect("the fixture nonce remains bounded");
        let admission = admission(nonce, index % 4 + 1);
        keys.push(admission.identity.raw.clone());
        runtime
            .admit(admission)
            .expect("every preexisting admission commits");
    }

    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let cache = Arc::new(TokioRwLock::new(ckb_verification::cache::init_cache()));
    let (cache_tx, _cache_rx) = mpsc::channel(TRANSACTIONS);
    let (_command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
    let cancel = CancellationToken::new();
    let handles = runtime
        .spawn_workers(&handle, cache, cache_tx, command_rx, cancel.clone())
        .expect("the validated topology reserves its handle vector");

    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let accepted = {
                let store = runtime.store.read();
                keys.iter()
                    .filter(|key| matches!(store.authority.entry(key), Some(OwnedTx::Accepted(_))))
                    .count()
            };
            if accepted == keys.len() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("role-specific wake-one batons must drain every preexisting head");

    cancel.cancel();
    for task in handles.tasks {
        task.handle
            .await
            .expect("authority worker task remains healthy")
            .expect("authority worker exits without a structural fault");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn runtime_single_any_verifier_settles_mixed_frontier_after_batons_are_consumed() {
    const TRANSACTIONS: usize = 32;

    let mut config = runtime_config();
    config.max_tx_verify_workers = 1;
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the single-verifier topology is valid");
    let large_cycles = config
        .max_tx_verify_cycles
        .checked_add(1)
        .expect("the fixture cycle declaration is bounded");
    let mut keys = Vec::new();
    keys.try_reserve(TRANSACTIONS)
        .expect("the bounded fixture reserves its key list");

    for index in 0..TRANSACTIONS {
        let nonce = 1_100u32
            .checked_add(u32::try_from(index).expect("the fixture count fits u32"))
            .expect("the fixture nonce remains bounded");
        let declared_cycles = if index % 2 == 0 { 0 } else { large_cycles };
        let admission = admission_with_cycles(nonce, index % 4 + 1, declared_cycles);
        keys.push(admission.identity.raw.clone());
        runtime
            .admit(admission)
            .expect("every mixed admission commits");
        let job = continued(
            runtime
                .try_checkout_for_foundation(WorkPermit::ResolveOnly)
                .expect("every resolution checkout remains healthy"),
        )
        .expect("the just-admitted resolution is ready");
        assert!(matches!(
            continued(
                runtime
                    .execute_compute(job)
                    .expect("every mixed resolution settles")
            ),
            AuthorityComputeOutcome::Settled
        ));
    }

    // Consume every coalesced hint without doing work. The sealed topology
    // must still recover from the authoritative levels through its initial
    // probe; notifications never become a second work authority.
    expect_signal(
        runtime.resolve_signal(),
        "coalesced admissions retain one Resolve hint",
    )
    .await;
    expect_signal(
        runtime.verify_small_signal(),
        "the mixed frontier retains one Small hint",
    )
    .await;
    // Depending on the fair-owner cursor, the current Any head may be the
    // same Small entry and therefore have no separate permit. Consume a
    // distinct Any permit when present without making duplicated publication
    // a requirement.
    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(10),
        runtime.verify_any_signal().notified(),
    )
    .await;

    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let cache = Arc::new(TokioRwLock::new(ckb_verification::cache::init_cache()));
    let (cache_tx, _cache_rx) = mpsc::channel(TRANSACTIONS);
    let (_command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
    let cancel = CancellationToken::new();
    let handles = runtime
        .spawn_workers(&handle, cache, cache_tx, command_rx, cancel.clone())
        .expect("the single-verifier worker set starts");

    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let pending = {
                let store = runtime.store.read();
                keys.iter()
                    .filter(|key| {
                        matches!(store.authority.entry(key), Some(OwnedTx::PreAccepted(_)))
                    })
                    .count()
            };
            if pending == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("one Any verifier must settle coalesced Small and Large levels");

    let (accepted, rejected) = {
        let store = runtime.store.read();
        let accepted = keys
            .iter()
            .filter(|key| matches!(store.authority.entry(key), Some(OwnedTx::Accepted(_))))
            .count();
        let rejected = keys
            .iter()
            .filter(|key| store.authority.entry(key).is_none())
            .count();
        (accepted, rejected)
    };
    assert_eq!(accepted, TRANSACTIONS / 2);
    assert_eq!(rejected, TRANSACTIONS / 2);

    cancel.cancel();
    for task in handles.tasks {
        task.handle
            .await
            .expect("authority worker task remains healthy")
            .expect("authority worker exits without a structural fault");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_worker_retains_rejected_settlement_until_effect_capacity_returns() {
    const EFFECT_BYTES: usize = 1024 * 1024;
    let mut config = runtime_config();
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
    .expect("the narrow fixture admits one effect in each region");
    let runtime = runtime_with_effect_limits(&config, snapshot, effects);

    queue_remote_rejection(&runtime, 907);

    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let cache = Arc::new(TokioRwLock::new(ckb_verification::cache::init_cache()));
    let (cache_tx, _cache_rx) = mpsc::channel(1);
    let (_command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
    let cancel = CancellationToken::new();
    let handles = runtime
        .spawn_workers(&handle, cache, cache_tx, command_rx, cancel.clone())
        .expect("the validated topology reserves its handle vector");

    let admission = admission(908, 98);
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("admission commits");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if matches!(
                runtime.store.read().authority.entry(&key),
                Some(OwnedTx::PreAccepted(entry))
                    if matches!(entry.phase, PreAcceptedPhase::Computing(_))
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the rejected settlement remains Computing while publication is full");

    let occupied_lease = runtime
        .wait_effect_checkout()
        .await
        .expect("effect checkout remains healthy")
        .expect("the occupied effect is queued");
    runtime
        .settle_effect(occupied_lease.complete_for_foundation().published())
        .expect("the occupied publication settles through the runtime boundary");

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if runtime.store.read().authority.entry(&key).is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the exact rejection commits after effect capacity returns");
    assert_eq!(
        runtime
            .store
            .read()
            .authority
            .resources()
            .preaccepted()
            .active_work,
        0
    );

    cancel.cancel();
    for task in handles.tasks {
        task.handle
            .await
            .expect("authority worker task remains healthy")
            .expect("authority worker exits without a structural fault");
    }
}

#[tokio::test]
async fn runtime_effect_boundary_retains_and_drains_a_closed_log_in_sequence() {
    let runtime = runtime();
    queue_remote_rejection(&runtime, 909);
    queue_remote_rejection(&runtime, 910);

    let first = runtime
        .wait_effect_checkout()
        .await
        .expect("the first checkout remains healthy")
        .expect("the first effect is committed");
    let first_sequence = first.sequence();
    runtime
        .close_effects()
        .expect("zero active compute permits effect close");
    assert!(!runtime.effects_closed_and_drained());
    assert_eq!(
        runtime.admit(admission(911, 99)).err(),
        Some(PlanError::EffectClosed),
        "closing the effect authority freezes new state producers"
    );

    runtime
        .settle_effect(first.retain())
        .expect("Retain returns the exact active capability to the head");
    let retained = runtime
        .wait_effect_checkout()
        .await
        .expect("retained checkout remains healthy")
        .expect("the retained head is still committed");
    assert_eq!(retained.sequence(), first_sequence);
    runtime
        .settle_effect(retained.complete_for_foundation().published())
        .expect("the retained head publishes exactly once");

    let second = runtime
        .wait_effect_checkout()
        .await
        .expect("the second checkout remains healthy")
        .expect("the second effect remains queued after close");
    assert!(second.sequence() > first_sequence);
    assert!(!runtime.effects_closed_and_drained());
    runtime
        .settle_effect(second.complete_for_foundation().circuit_disposed())
        .expect("a stable endpoint circuit may dispose its exact batch");

    assert!(
        runtime
            .wait_effect_checkout()
            .await
            .expect("the drained observation remains healthy")
            .is_none()
    );
    assert!(runtime.effects_closed_and_drained());
}

#[tokio::test]
async fn runtime_effect_close_wakes_an_idle_level_waiter() {
    let runtime = runtime();
    let waiter = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.wait_effect_checkout().await })
    };
    tokio::task::yield_now().await;

    runtime
        .close_effects()
        .expect("an idle authority closes without a synthetic effect");
    let checkout = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("close cannot lose the idle publisher wake")
        .expect("the publisher task remains healthy")
        .expect("effect checkout remains healthy");
    assert!(checkout.is_none());
    assert!(runtime.effects_closed_and_drained());
}

#[tokio::test]
async fn runtime_waiter_wakes_after_post_commit_admission_publication() {
    let runtime = runtime();
    let waiter = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            let cancel = CancellationToken::new();
            runtime
                .wait_checkout(WorkPermit::ResolveOnly, &cancel)
                .await
        })
    };
    tokio::task::yield_now().await;

    runtime
        .admit(admission(902, 92))
        .expect("admission commits before publication");
    let job = continued(
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("the post-commit wake cannot be lost")
            .expect("the waiter task remains healthy")
            .expect("the authority runtime remains healthy"),
    )
    .expect("the waiter was not cancelled");
    assert!(matches!(
        runtime.settle_compute(retry(job), SettlementOrigin::Completion),
        ControlFlow::Continue(())
    ));
}

#[test]
fn runtime_capture_failure_requeues_before_returning_the_typed_error() {
    let runtime = runtime();
    let admission = admission(903, 93);
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("admission commits");
    {
        let mut store = runtime.store.write();
        store.authority.force_chain_view(ChainViewId::new(
            ChainRevision(1),
            ckb_types::packed::Byte32::new([0x93; 32]),
        ));
    }

    assert!(matches!(
        runtime.try_checkout_for_foundation(WorkPermit::ResolveOnly),
        Err(FoundationCheckoutError::Authority(
            AuthorityComputeError::Resolution(
                super::super::resolver::ResolutionExecutionKind::StaleView
            )
        ))
    ));
    assert!(is_queued_resolve(&runtime, &key));
}

#[test]
fn runtime_configuration_builds_every_authority_policy_together() {
    let config = runtime_config();
    let consensus = ConsensusBuilder::default().build();
    let runtime = AuthorityRuntimeConfig::from_runtime(&config, &consensus)
        .expect("the production fixture compiles into one authority policy");
    let limit = runtime.resources.preaccepted_limit_for_foundation();
    assert_eq!(
        limit.total_bytes(),
        Some(config.tx_pipeline_resident_size_budget()),
        "retained ownership and every simultaneous compute reservation share one physical ceiling"
    );
    assert!(limit.compute_bytes() > 0);
    assert!(limit.compute_edges() > 0);
    assert!(limit.bytes < config.tx_pipeline_resident_size_budget());
}

#[test]
fn runtime_configuration_rejects_an_unusable_pipeline_budget() {
    let mut config = runtime_config();
    config.max_tx_pipeline_resident_size = PREACCEPTED_ENTRY_BYTES - 1;
    let consensus = ConsensusBuilder::default().build();
    assert_eq!(
        AuthorityRuntimeConfig::from_runtime(&config, &consensus).err(),
        Some(RuntimeConfigError::PipelineBudgetTooSmall)
    );
}

#[test]
fn runtime_configuration_rejects_an_unusable_per_work_grant() {
    let mut config = runtime_config();
    config.max_tx_pipeline_resident_size = 1_000_000;
    config.max_tx_verify_workers = 10_000;
    let consensus = ConsensusBuilder::default().build();
    assert_eq!(
        AuthorityRuntimeConfig::from_runtime(&config, &consensus).err(),
        Some(RuntimeConfigError::PipelineBudgetTooSmall)
    );
}

#[test]
fn runtime_configuration_rejects_effect_capacity_arithmetic_overflow() {
    let mut config = runtime_config();
    config.max_tx_pool_size = usize::MAX;
    config.max_tx_pool_resident_size = usize::MAX;
    let consensus = ConsensusBuilder::default().build();
    assert_eq!(
        AuthorityRuntimeConfig::from_runtime(&config, &consensus).err(),
        Some(RuntimeConfigError::Arithmetic)
    );
}

#[test]
fn runtime_configuration_error_conversions_preserve_failure_domains() {
    assert_eq!(
        runtime_resource_config_error(ResourceConfigError::TransientComputeOverflow),
        RuntimeConfigError::Arithmetic
    );
    assert_eq!(
        runtime_resource_config_error(ResourceConfigError::LimitHierarchy),
        RuntimeConfigError::ResourceConfiguration
    );
    assert_eq!(
        runtime_authority_config_error(
            AuthorityConfigError::Effect(EffectConfigError::Arithmetic,)
        ),
        RuntimeConfigError::Arithmetic
    );
    assert_eq!(
        runtime_authority_config_error(
            AuthorityConfigError::Effect(EffectConfigError::Allocation,)
        ),
        RuntimeConfigError::AuthorityAllocation
    );
}
