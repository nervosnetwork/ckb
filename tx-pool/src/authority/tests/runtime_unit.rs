use super::{
    AuthorityAdministrationError, AuthorityComputeCompletion, AuthorityComputeExchangeFollowUp,
    AuthorityComputeJob, AuthorityComputeOutcome, AuthorityDirectAdmissionError,
    AuthorityDirectRejectionExecution, AuthorityDirectResolutionOutcome, AuthorityDriverError,
    AuthorityMaintenanceOutcome, AuthorityReadyCommitLane, AuthorityReadyCommitTerminal,
    AuthorityReadyDispatch, AuthorityReadyOutcome, AuthorityRuntime, AuthorityRuntimeConfig,
    FinalAdmissionCaptureError, PREACCEPTED_ENTRY_BYTES, PlanError, ReadyDisposition,
    ReadyPlanInput, ReadyValidationError, ReservedReadyPlanInput,
    RetainedIngressBatchFailureReason, RuntimeConfigError, SettlementOrigin,
    accepted_validity_transition, runtime_authority_config_error, runtime_resource_config_error,
    test_support::{AuthorityComputeError, AuthorityComputeSettlement, FoundationCheckoutError},
};
use crate::authority::chain::AcceptedValidityTransition;
use crate::authority::effect::{
    CommittedEffect, CommittedRejection, EffectBatchBound, EffectBatchBounds, EffectCapacity,
    EffectConfigError, EffectLimits, EffectPolicy, RejectionAudience,
};
use crate::authority::exchange::{ComputeVerifierSlot, ComputeWorkerGrant, ComputeWorkerSlot};
use crate::authority::ingress::{
    BoundedTransaction, RetainedAdmissionBatch, RetainedIngressAttempt, proposal,
    test_support::remote_at_for_foundation,
};
use crate::authority::plan::{
    AuthorityConfigError, AuthorityFault, Backpressure, ComputeExchangeCompletion,
    ComputeExchangeDeferredRoute, ComputeSettlementRecovery, StalePlan,
};
use crate::authority::resources::{
    AcceptedResources, ComputeLimits, ResourceConfigError, ResourceLimits, ResourceVector,
};
use crate::authority::scheduler::ReadyReservation;
use crate::authority::shard::{ConcurrentRemovalProbe, SharedIngressProbePhase};
use crate::authority::state::{
    AcceptedAtMillis, ChainRevision, ChainViewId, OwnedTx, PreAcceptedPhase, PreAcceptedSource,
    QueuedWork, RawTxHash, RemoteResidencyLease, ValidatedAdmission, VerifyCapability, WorkPermit,
    test_support::RejectionKind,
};
use crate::authority::tests::foundation::{
    accepted_parent_child_at, accepted_parent_with_ready_children, add_leaf_rbf_pair,
    admit_remote_until, independent_batch, missing_keys,
};
use crate::authority::validation::FinalAdmissionValidationError;
use crate::authority::work::CheckedOutWork;
use crate::authority::worker::{AuthorityWorkerRole, test_support::AuthorityTestWorkerOwner};
use crate::constants::MAX_READY_BATCH;
use crate::error::Reject;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_async_runtime::Handle;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_network::PeerIndex;
use ckb_script::{ChunkCommand, TxPoolVmExecutionMode};
use ckb_snapshot::Snapshot;
use ckb_stop_handler::CancellationToken;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    U256,
    core::{FeeRate, TransactionBuilder},
    prelude::{Pack, Unpack},
};
use ckb_verification::cache::ScriptVerificationRules;
use std::ops::ControlFlow;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{RwLock as TokioRwLock, mpsc};

fn runtime_config() -> TxPoolConfig {
    TxPoolConfig {
        max_tx_pool_size: 180_000_000,
        max_tx_pool_resident_size: 1_000_000_000,
        min_fee_rate: FeeRate::zero(),
        min_rbf_rate: FeeRate::zero(),
        max_tx_verify_cycles: 70_000_000,
        min_tx_verify_time_ms: 250,
        tx_verify_cycles_per_ms: 10_000,
        max_tx_verify_time_ms: 30_000,
        max_tx_verify_initial_load_bytes: 256 * 1024 * 1024,
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
    AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production authority runtime fixture is valid")
}

#[test]
fn runtime_accepted_validity_obeys_the_complete_priority_truth_table() {
    let rules = [
        ScriptVerificationRules::V0,
        ScriptVerificationRules::V1,
        ScriptVerificationRules::V2,
    ];
    for old_rules in rules {
        for new_rules in rules {
            for had_detached_chain in [false, true] {
                let production =
                    accepted_validity_transition(old_rules, new_rules, had_detached_chain);
                let expected = if old_rules != new_rules {
                    AcceptedValidityTransition::RulesChanged
                } else if had_detached_chain {
                    AcceptedValidityTransition::ContextChanged
                } else {
                    AcceptedValidityTransition::Preserved
                };
                assert_eq!(production, expected);
            }
        }
    }
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

fn runtime_with_one_effect_batch() -> AuthorityRuntime {
    const EFFECT_BYTES: usize = 1024 * 1024;
    let snapshot = genesis_snapshot();
    let effects = EffectLimits::partitioned(
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectBatchBounds::new(
            EffectBatchBound::new(MAX_READY_BATCH, EFFECT_BYTES),
            EffectBatchBound::new(MAX_READY_BATCH, EFFECT_BYTES),
            EffectBatchBound::new(MAX_READY_BATCH, EFFECT_BYTES),
        ),
    )
    .expect("the narrow fixture admits one bounded effect batch");
    runtime_with_effect_limits(&runtime_config(), snapshot, effects)
}

fn runtime_with_one_remote_effect_batch_and_trusted_headroom() -> AuthorityRuntime {
    const EFFECT_BYTES: usize = 1024 * 1024;
    let snapshot = genesis_snapshot();
    let effects = EffectLimits::partitioned(
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectCapacity::new(2, EFFECT_BYTES * 2),
        EffectCapacity::new(2, EFFECT_BYTES * 2),
        EffectBatchBounds::new(
            EffectBatchBound::new(MAX_READY_BATCH, EFFECT_BYTES),
            EffectBatchBound::new(MAX_READY_BATCH, EFFECT_BYTES),
            EffectBatchBound::new(MAX_READY_BATCH, EFFECT_BYTES),
        ),
    )
    .expect("the trust-boundary fixture isolates remote from trusted headroom");
    runtime_with_effect_limits(&runtime_config(), snapshot, effects)
}

fn runtime_with_one_accepted_owner() -> AuthorityRuntime {
    let snapshot = genesis_snapshot();
    let resources = ResourceLimits::new(
        ResourceVector::new(16, 128 * 1024, 128, 16),
        ResourceVector::new(16, 128 * 1024, 128, 16),
        ResourceVector::new(2, 16 * 1024, 16, 2),
        AcceptedResources::new(1, 64 * 1024, 64 * 1024, 64),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .and_then(|limits| {
        limits.with_replacement_history_limit(ResourceVector::new(4, 32 * 1024, 32, 0))
    })
    .expect("the one-accepted Ready fixture has a valid resource hierarchy");
    AuthorityRuntime::new_with_resource_limits_for_foundation(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
        resources,
    )
    .expect("the one-accepted Ready runtime reserves every bounded projection")
}

fn queue_remote_rejection(runtime: &AuthorityRuntime, nonce: u32) {
    queue_rejection(runtime, EffectPolicy::Remote, nonce);
}

fn queue_rejection(runtime: &AuthorityRuntime, policy: EffectPolicy, nonce: u32) {
    let publication = {
        let store = runtime.store.read();
        store
            .authority
            .effect_publication_for_foundation(
                policy,
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
    assert_eq!(runtime.publish_committed(retirement), None);
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

fn completion(outcome: AuthorityComputeOutcome) -> AuthorityComputeCompletion {
    let AuthorityComputeOutcome::Completion(completion) = outcome else {
        panic!("the fixture produces a terminal compute completion")
    };
    completion
}

async fn advance_admission_to_ready(
    runtime: &AuthorityRuntime,
    admission: ValidatedAdmission,
) -> RawTxHash {
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("admission commits");
    let job = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveThenVerify(VerifyCapability::Any))
            .expect("checkout remains healthy"),
    )
    .expect("resolve work is ready");
    let AuthorityComputeOutcome::Verification(request) = runtime.execute_compute(job) else {
        panic!("the empty-script fixture fits continuous verification")
    };
    let cache = ckb_verification::cache::init_cache();
    let (_command_tx, mut command_rx) =
        tokio::sync::watch::channel(ckb_script::ChunkCommand::Resume);
    let completed = runtime
        .execute_verification(request.bind_cache(&cache), &mut command_rx)
        .await;
    let verification = continued(runtime.settle_completion(completed));
    assert!(verification.into_parts().1.is_some());
    key
}

async fn advance_remote_to_ready(runtime: &AuthorityRuntime, nonce: u32, peer: usize) -> RawTxHash {
    advance_admission_to_ready(runtime, admission(nonce, peer)).await
}

fn reserve_excluding_one_compatible_ready_wave(
    runtime: &AuthorityRuntime,
    hashes: &[RawTxHash],
) -> ([RawTxHash; 2], ReadyReservation) {
    runtime.with_authority_for_foundation(|authority| {
        let mut selected = None;
        'left: for left_index in 0..hashes.len() {
            let left_batch =
                independent_batch(authority, std::slice::from_ref(&hashes[left_index]));
            let Some(left) = authority
                .compile_shared_independent_settlement(&left_batch)
                .expect("a singleton Ready candidate compiles")
                .into_option_for_foundation()
            else {
                continue;
            };
            let left_support = left.physical_apply_support_for_foundation();
            drop(left);
            for (right_index, right_hash) in
                hashes.iter().enumerate().skip(left_index.saturating_add(1))
            {
                let right_batch = independent_batch(authority, std::slice::from_ref(right_hash));
                let Some(right) = authority
                    .compile_shared_independent_settlement(&right_batch)
                    .expect("a later singleton Ready candidate compiles")
                    .into_option_for_foundation()
                else {
                    continue;
                };
                let right_support = right.physical_apply_support_for_foundation();
                if left_support.is_compatible(right_support) {
                    selected = Some((left_index, right_index));
                    drop(right);
                    break 'left;
                }
                drop(right);
            }
        }
        let (left, right) =
            selected.expect("the fixed test layout contains two compatible Ready cuts");
        let selected = [hashes[left].clone(), hashes[right].clone()];
        let excluded = hashes
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != left && *index != right)
            .map(|(_, hash)| hash.clone())
            .collect::<Vec<_>>();
        let excluded = authority.reserve_ready_exact_for_foundation(&excluded);
        (selected, excluded)
    })
}

fn reserve_excluding_one_compatible_ready_triple(
    runtime: &AuthorityRuntime,
    hashes: &[RawTxHash],
) -> [RawTxHash; 3] {
    let selected = runtime.with_authority_for_foundation(|authority| {
        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(hashes.len())
            .expect("the bounded triple support table allocates");
        for hash in hashes {
            let batch = independent_batch(authority, std::slice::from_ref(hash));
            let Some(compiled) = authority
                .compile_shared_independent_settlement(&batch)
                .expect("a singleton Ready candidate compiles")
                .into_option_for_foundation()
            else {
                continue;
            };
            candidates.push((
                hash.clone(),
                compiled.physical_apply_support_for_foundation(),
            ));
        }
        let mut selected = None;
        'outer: for left in 0..candidates.len() {
            for middle in left.saturating_add(1)..candidates.len() {
                if !candidates[left].1.is_compatible(candidates[middle].1) {
                    continue;
                }
                for right in middle.saturating_add(1)..candidates.len() {
                    if candidates[left].1.is_compatible(candidates[right].1)
                        && candidates[middle].1.is_compatible(candidates[right].1)
                    {
                        selected = Some([
                            candidates[left].0.clone(),
                            candidates[middle].0.clone(),
                            candidates[right].0.clone(),
                        ]);
                        break 'outer;
                    }
                }
            }
        }
        selected.expect("the bounded fixture contains a compatible Ready triple")
    });
    remove_unselected_ready_fixture(runtime, hashes.iter(), hashes, &selected);
    selected
}

fn reserve_excluding_one_compatible_cross_class_ready_wave(
    runtime: &AuthorityRuntime,
    remote_hashes: &[RawTxHash],
    trusted_hashes: &[RawTxHash],
) -> [RawTxHash; 2] {
    let selected = runtime.with_authority_for_foundation(|authority| {
        let mut selected = None;
        'remote: for remote in remote_hashes {
            let remote_batch = independent_batch(authority, std::slice::from_ref(remote));
            let Some(remote_compiled) = authority
                .compile_shared_independent_settlement(&remote_batch)
                .expect("a remote singleton Ready candidate compiles")
                .into_option_for_foundation()
            else {
                continue;
            };
            for trusted in trusted_hashes {
                let trusted_batch = independent_batch(authority, std::slice::from_ref(trusted));
                let Some(trusted_compiled) = authority
                    .compile_shared_independent_settlement(&trusted_batch)
                    .expect("a trusted singleton Ready candidate compiles")
                    .into_option_for_foundation()
                else {
                    continue;
                };
                if remote_compiled.is_compatible_with(&trusted_compiled) {
                    selected = Some([trusted.clone(), remote.clone()]);
                    drop(trusted_compiled);
                    break 'remote;
                }
                drop(trusted_compiled);
            }
            drop(remote_compiled);
        }
        selected.expect("the bounded layout contains one cross-class shard pair")
    });
    remove_unselected_ready_fixture(
        runtime,
        remote_hashes.iter().chain(trusted_hashes),
        remote_hashes,
        &selected,
    );
    selected
}

fn drive_ready_while_outer_reader_is_held(
    runtime: &AuthorityRuntime,
    timeout: std::time::Duration,
) -> Result<AuthorityReadyDispatch, AuthorityDriverError> {
    let (reader_entered_tx, reader_entered_rx) = std::sync::mpsc::channel();
    let (release_reader_tx, release_reader_rx) = std::sync::mpsc::channel();
    let (dispatch_tx, dispatch_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let reader_runtime = runtime.clone();
        let reader = scope.spawn(move || {
            reader_runtime.with_authority_read_for_foundation(|_| {
                let _ = reader_entered_tx.send(());
                let _ = release_reader_rx.recv();
            });
        });
        reader_entered_rx
            .recv_timeout(timeout)
            .expect("the unrelated outer reader is held");
        let driver_runtime = runtime.clone();
        let driver = scope.spawn(move || {
            let _ = dispatch_tx.send(driver_runtime.try_drive_ready());
        });
        let dispatch = dispatch_rx.recv_timeout(timeout);
        release_reader_tx
            .send(())
            .expect("release the unrelated outer reader");
        reader.join().expect("the outer reader does not panic");
        driver.join().expect("the Ready driver does not panic");
        dispatch.expect("Ready compilation cannot require the unrelated outer writer")
    })
}

fn remove_unselected_ready_fixture<'hash, const N: usize>(
    runtime: &AuthorityRuntime,
    hashes: impl IntoIterator<Item = &'hash RawTxHash>,
    remote_hashes: &[RawTxHash],
    selected: &[RawTxHash; N],
) {
    for hash in hashes {
        if selected.contains(hash) {
            continue;
        }
        assert!(
            runtime
                .remove_local_transaction(&hash.0)
                .expect("fixture pruning uses the production administrative path")
        );
        let receipt = runtime.with_authority_for_foundation(|authority| {
            authority.effect_publication_receipt_for_foundation()
        });
        if remote_hashes.contains(hash) {
            let receipt = receipt
                .expect("each independent remote fixture removal publishes one exact release");
            assert!(matches!(
                receipt.effects(),
                [CommittedEffect::RemoteIngressReleased(release)] if release.tx_hash() == hash
            ));
            runtime
                .settle_effect_for_foundation(receipt.complete_for_foundation().published())
                .expect("fixture pruning settles the exact release before the next removal");
        } else {
            assert!(
                receipt.is_none(),
                "trusted fixture pruning cannot invent a remote release"
            );
        }
    }
    let effects = runtime.effect_observation_for_foundation();
    assert_eq!(
        (effects.total_usage.batches, effects.total_usage.bytes),
        (0, 0)
    );
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
        AuthorityDriverError::Fault(AuthorityFault::SchedulerProjection)
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
fn runtime_resolution_uses_assembled_policy_and_returns_linear_completion() {
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
    let completion = completion(runtime.execute_compute(job));
    {
        let store = runtime.store.read();
        assert!(matches!(
            store.authority.entry(&key),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Computing(_))
        ));
    }
    let aftermath = continued(runtime.settle_completion(completion));
    let (origin, cache_update) = aftermath.into_parts();
    assert!(matches!(origin, SettlementOrigin::Completion));
    assert!(cache_update.is_none());
    let store = runtime.store.read();
    assert!(matches!(
        store.authority.entry(&key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
    ));
}

#[test]
fn runtime_completion_only_exchange_commits_while_store_read_is_held() {
    const EXCHANGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    let runtime = runtime();
    let admission = admission(1_067, 107);
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("admission commits");
    let job = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveOnly)
            .expect("checkout remains healthy"),
    )
    .expect("resolve work is ready");
    let completion = ComputeExchangeCompletion::from_finished(
        ComputeWorkerSlot::ordered_resolve(),
        completion(runtime.execute_compute(job)).finish_execution(),
    );

    let held_read = runtime.store.read();
    let worker_runtime = runtime.clone();
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let summary = match worker_runtime.exchange_compute(vec![completion], Vec::new()) {
            Ok(committed) => {
                let summary = (
                    committed.settled.len(),
                    committed.obsolete.len(),
                    committed.deferred.len(),
                    committed.assignments.len(),
                    committed.unused_grants.len(),
                    committed.follow_up,
                );
                drop(committed);
                Ok(summary)
            }
            Err(error) => {
                drop(error);
                Err(())
            }
        };
        result_tx
            .send(summary)
            .expect("the runtime exchange observer remains alive");
    });

    let result = result_rx.recv_timeout(EXCHANGE_TIMEOUT);
    drop(held_read);
    worker.join().expect("the exchange worker remains healthy");
    let (settled, obsolete, deferred, assignments, unused_grants, follow_up) = result
        .expect("completion-only exchange cannot require the outer AuthorityStore write guard")
        .expect("the owner-local completion commits through the shared path");
    assert_eq!(settled, 1);
    assert_eq!(obsolete, 0);
    assert_eq!(deferred, 0);
    assert_eq!(assignments, 0);
    assert_eq!(unused_grants, 0);
    assert_eq!(follow_up, AuthorityComputeExchangeFollowUp::None);

    let store = runtime.store.read();
    assert!(matches!(
        store.authority.entry(&key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
    ));
}

#[test]
fn runtime_completion_refill_exchange_commits_while_store_read_is_held() {
    const EXCHANGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    let runtime = runtime();
    let first = admission(1_070, 110);
    let first_key = first.identity.raw.clone();
    runtime.admit(first).expect("the completing owner commits");
    let second = admission(1_071, 111);
    let second_key = second.identity.raw.clone();
    runtime.admit(second).expect("the refill owner commits");
    let job = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveOnly)
            .expect("checkout remains healthy"),
    )
    .expect("resolve work is ready");
    let slot = ComputeWorkerSlot::ordered_resolve();
    let completion = ComputeExchangeCompletion::from_finished(
        slot,
        completion(runtime.execute_compute(job)).finish_execution(),
    );
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the finished completion returns one fair execution permit");
    let grant = ComputeWorkerGrant::new(slot, execution);

    let held_read = runtime.store.read();
    let worker_runtime = runtime.clone();
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        result_tx
            .send(worker_runtime.exchange_compute(vec![completion], vec![grant]))
            .expect("the runtime exchange observer remains alive");
    });
    let result = result_rx.recv_timeout(EXCHANGE_TIMEOUT);
    drop(held_read);
    worker.join().expect("the exchange worker remains healthy");
    let committed = result
        .expect("completion plus refill cannot require the outer AuthorityStore write guard")
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the owner-local completion and shared fair checkout commit")
        });
    assert_eq!(committed.settled.len(), 1);
    assert_eq!(committed.obsolete.len(), 0);
    assert_eq!(committed.deferred.len(), 0);
    assert_eq!(committed.assignments.len(), 1);
    assert!(committed.capture_failures.is_empty());
    assert!(committed.unused_grants.is_empty());
    assert_eq!(committed.follow_up, AuthorityComputeExchangeFollowUp::None);
    let assignment = committed
        .assignments
        .into_iter()
        .next()
        .expect("one fair grant checks out the remaining Resolve owner");
    let (_, job) = assignment.into_parts();
    assert!(matches!(
        runtime.store.read().authority.entry(&first_key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
    ));
    assert!(matches!(
        runtime.store.read().authority.entry(&second_key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    assert!(matches!(
        runtime.settle_compute(job.retry_for_foundation(), SettlementOrigin::Completion),
        ControlFlow::Continue(())
    ));
}

#[test]
fn runtime_remote_expiry_commits_while_an_unrelated_outer_reader_is_held() {
    const OUTER_READER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    let snapshot = genesis_snapshot();
    let mut config = runtime_config();
    config.expiry_hours = 0;
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the expiry runtime fixture is valid");
    let expired = runtime
        .with_authority_for_foundation(|authority| admit_remote_until(authority, 3_001, 401, 0));

    let held_reader = runtime.store.read();
    let worker_runtime = runtime.clone();
    let (finished, observed) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        finished
            .send(worker_runtime.expire_remote_due())
            .expect("the expiry observer remains alive");
    });
    let outcome = observed
        .recv_timeout(OUTER_READER_TIMEOUT)
        .expect("Remote expiry cannot require the unrelated outer writer");
    drop(held_reader);
    worker
        .join()
        .expect("the Remote-expiry worker remains healthy");
    assert_eq!(
        outcome.expect("the shared Remote-expiry step remains healthy"),
        AuthorityMaintenanceOutcome::Applied
    );
    assert!(runtime.store.read().authority.entry(&expired).is_none());
    runtime.with_authority_read_for_foundation(|authority| {
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn runtime_accepted_expiry_commits_while_an_unrelated_outer_reader_is_held() {
    const OUTER_READER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    let snapshot = genesis_snapshot();
    let mut config = runtime_config();
    config.expiry_hours = 0;
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the Accepted-expiry runtime fixture is valid");
    let (parent, child) = runtime.with_authority_for_foundation(|authority| {
        accepted_parent_child_at(authority, 93, AcceptedAtMillis(0), AcceptedAtMillis(1))
    });

    let held_reader = runtime.store.read();
    let worker_runtime = runtime.clone();
    let (finished, observed) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        finished
            .send(worker_runtime.expire_accepted_due())
            .expect("the Accepted-expiry observer remains alive");
    });
    let outcome = observed
        .recv_timeout(OUTER_READER_TIMEOUT)
        .expect("Accepted expiry cannot require the unrelated outer writer");
    drop(held_reader);
    worker
        .join()
        .expect("the Accepted-expiry worker remains healthy");
    assert_eq!(
        outcome.expect("the shared Accepted-expiry step remains healthy"),
        AuthorityMaintenanceOutcome::Applied
    );
    let store = runtime.store.read();
    assert!(store.authority.entry(&parent).is_none());
    assert!(store.authority.entry(&child).is_none());
    assert!(store.authority.primary_projection_consistent());
}

#[test]
fn runtime_waiting_exchange_and_exact_settlement_need_no_outer_writer() {
    const OUTER_READER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    let runtime = runtime();
    let admission = admission(1_073, 113);
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("the Remote owner commits");
    let job = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveOnly)
            .expect("checkout remains healthy"),
    )
    .expect("resolve work is ready");
    let slot = ComputeWorkerSlot::ordered_resolve();
    let completion = job
        .missing_for_foundation(missing_keys())
        .into_exchange_completion_for_foundation(slot);
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the finished completion returns one fair execution permit");
    let grant = ComputeWorkerGrant::new(slot, execution);

    let held_exchange_read = runtime.store.read();
    let exchange_runtime = runtime.clone();
    let (exchange_tx, exchange_rx) = std::sync::mpsc::sync_channel(1);
    let exchange_worker = std::thread::spawn(move || {
        exchange_tx
            .send(exchange_runtime.exchange_compute(vec![completion], vec![grant]))
            .expect("the exchange observer remains alive");
    });
    let exchange_before_release = exchange_rx.recv_timeout(OUTER_READER_TIMEOUT);
    let exchange_finished_while_reader_held = exchange_before_release.is_ok();
    drop(held_exchange_read);
    let committed = match exchange_before_release {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => exchange_rx
            .recv()
            .expect("exchange finishes after the predecessor outer reader is released"),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("the exchange worker remains connected")
        }
    }
    .unwrap_or_else(|failure| {
        drop(failure);
        panic!("the exact nonlocal completion remains recoverable")
    });
    exchange_worker
        .join()
        .expect("the exchange worker remains healthy");
    assert!(committed.settled.is_empty());
    assert!(committed.obsolete.is_empty());
    assert_eq!(committed.deferred.len(), 1);
    assert!(committed.assignments.is_empty());
    assert!(committed.capture_failures.is_empty());
    assert_eq!(committed.unused_grants.len(), 1);
    assert_eq!(committed.follow_up, AuthorityComputeExchangeFollowUp::None);
    let deferred = committed
        .deferred
        .into_iter()
        .next()
        .expect("the nonlocal completion keeps one exact route");
    let (route, completion) = deferred.into_parts();
    assert_eq!(
        route,
        crate::authority::plan::ComputeExchangeDeferredRoute::ExactSettlement
    );
    let (returned_slot, finished) = completion.into_parts();
    assert_eq!(returned_slot, slot);

    let held_settlement_read = runtime.store.read();
    let settlement_runtime = runtime.clone();
    let (settlement_tx, settlement_rx) = std::sync::mpsc::sync_channel(1);
    let settlement_worker = std::thread::spawn(move || {
        settlement_tx
            .send(settlement_runtime.settle_finished(finished))
            .expect("the exact-settlement observer remains alive");
    });
    let settlement_before_release = settlement_rx.recv_timeout(OUTER_READER_TIMEOUT);
    let settlement_finished_while_reader_held = settlement_before_release.is_ok();
    drop(held_settlement_read);
    let settlement = match settlement_before_release {
        Ok(settlement) => settlement,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => settlement_rx
            .recv()
            .expect("exact settlement finishes after the predecessor reader is released"),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("the settlement worker remains connected")
        }
    };
    settlement_worker
        .join()
        .expect("the exact-settlement worker remains healthy");
    match settlement {
        ControlFlow::Continue(committed) => {
            let (aftermath, post_commit_fault) = committed.into_parts();
            assert_eq!(post_commit_fault, None);
            drop(aftermath);
        }
        ControlFlow::Break(pending) => {
            panic!("shared exact settlement failed: {:?}", pending.recovery())
        }
    }

    assert!(
        exchange_finished_while_reader_held && settlement_finished_while_reader_held,
        "ordinary outer writers remain: nonlocal_exchange={exchange_finished_while_reader_held}, exact_settlement={settlement_finished_while_reader_held}"
    );
    assert!(matches!(
        runtime.store.read().authority.entry(&key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Waiting(_))
    ));
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
}

#[tokio::test]
async fn runtime_exact_settlement_contention_rolls_back_effect_and_retries_linearly() {
    const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    let runtime = runtime();
    let admission = admission(1_074, 114);
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("the Remote owner commits");
    let job = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveOnly)
            .expect("checkout remains healthy"),
    )
    .expect("resolve work is ready");
    let slot = ComputeWorkerSlot::ordered_resolve();
    let (_, finished) = job
        .missing_for_foundation(missing_keys())
        .into_exchange_completion_for_foundation(slot)
        .into_parts();
    let dependency = missing_keys()
        .into_iter()
        .next()
        .expect("the foundation frontier is non-empty");
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_read_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_compute_settlement_commit_probe(Some(probe));
    });
    let capacity_notified = runtime.effect_capacity_signal().notified();
    tokio::pin!(capacity_notified);
    let _ = capacity_notified.as_mut().enable();

    let settlement_runtime = runtime.clone();
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        result_tx
            .send(settlement_runtime.settle_finished(finished))
            .expect("the settlement observer remains alive");
    });
    entered
        .recv_timeout(PROBE_TIMEOUT)
        .expect("the exact settlement stages its effect before the final owner cut");
    runtime.with_authority_read_for_foundation(|authority| {
        authority
            .apply_dependency_loss_during_shared_plan_for_foundation(vec![dependency])
            .expect("the real dependency frontier advances independently");
    });
    release
        .send(())
        .expect("the stale exact settlement resumes");
    let pending = result_rx
        .recv_timeout(PROBE_TIMEOUT)
        .expect("the contended settlement returns without a global retry")
        .break_value()
        .expect("the exact owner remains Computing and must be retried");
    worker
        .join()
        .expect("the contended settlement worker remains healthy");
    runtime.with_authority_read_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_compute_settlement_commit_probe(None);
    });
    assert!(matches!(
        pending.recovery(),
        ComputeSettlementRecovery::RetryExact(_)
    ));
    tokio::time::timeout(PROBE_TIMEOUT, capacity_notified.as_mut())
        .await
        .expect("explicit staged-effect rollback publishes its released capacity");

    let (failure, aftermath) = pending.into_parts();
    let retry = super::AuthorityFinishedCompute::from_parts(failure.into_settlement(), aftermath);
    let committed = continued(runtime.settle_finished(retry));
    let (aftermath, post_commit_fault) = committed.into_parts();
    assert_eq!(post_commit_fault, None);
    drop(aftermath);
    assert!(matches!(
        runtime.store.read().authority.entry(&key),
        Some(OwnedTx::PreAccepted(entry))
            if !matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
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
}

#[test]
fn runtime_effect_settlement_commits_while_store_read_is_held() {
    const SETTLEMENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    let runtime = runtime();
    let admission = admission(1_072, 112);
    let key = admission.identity.raw.clone();
    runtime.admit(admission).expect("admission commits");
    assert!(
        runtime
            .remove_local_transaction(&key.0)
            .expect("remote administrative removal remains coherent")
    );
    let receipt = runtime
        .with_authority_for_foundation(|authority| {
            authority.effect_publication_receipt_for_foundation()
        })
        .expect("remote removal publishes one exact release");
    let settlement = receipt.complete_for_foundation().published();

    let held_read = runtime.store.read();
    let worker_runtime = runtime.clone();
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        result_tx
            .send(worker_runtime.settle_effect_for_foundation(settlement))
            .expect("the effect settlement observer remains alive");
    });

    let result = result_rx.recv_timeout(SETTLEMENT_TIMEOUT);
    drop(held_read);
    worker
        .join()
        .expect("the effect settlement worker remains healthy");
    result
        .expect("effect-only settlement cannot require the outer AuthorityStore write guard")
        .expect("the exact effect lease settles");
    let effects = runtime.effect_observation_for_foundation();
    assert!(effects.queued.is_empty());
    assert_eq!(
        (effects.total_usage.batches, effects.total_usage.bytes),
        (0, 0)
    );
}

#[test]
fn runtime_new_proposal_ingress_commits_while_store_read_is_held() {
    const INGRESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    let runtime = runtime();
    let consensus = ConsensusBuilder::default().build();
    let transaction = TransactionBuilder::default()
        .input(ckb_types::packed::CellInput::new(
            ckb_types::packed::OutPoint::new(ckb_types::packed::Byte32::new([0x73; 32]), 0),
            0,
        ))
        .output(ckb_types::packed::CellOutput::default())
        .output_data(ckb_types::bytes::Bytes::new().pack())
        .build();
    let key = RawTxHash(transaction.hash());
    let bounded = BoundedTransaction::try_new(transaction)
        .expect("the proposal fixture transaction is bounded");
    let attempt = proposal(bounded, &consensus);
    assert!(matches!(
        &attempt,
        crate::authority::ingress::RetainedIngressAttempt::Validated(_)
    ));
    let batch = RetainedAdmissionBatch::new(attempt, std::collections::VecDeque::new())
        .expect("one proposal attempt is a homogeneous batch");

    let held_read = runtime.store.read();
    let worker_runtime = runtime.clone();
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        result_tx
            .send(worker_runtime.commit_retained_ingress_batch(batch))
            .expect("the retained-ingress observer remains alive");
    });

    let result = result_rx.recv_timeout(INGRESS_TIMEOUT);
    drop(held_read);
    worker
        .join()
        .expect("the retained-ingress worker remains healthy");
    let (consumed, remaining, post_commit_fault) = result
        .expect("new Proposal insertion cannot require the outer AuthorityStore write guard")
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the all-new Proposal insertion commits through the shared route")
        });
    assert_eq!(consumed, 1);
    assert!(remaining.is_empty());
    assert_eq!(post_commit_fault, None);
    assert!(matches!(
        runtime.store.read().authority.entry(&key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Proposal { .. })
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
}

#[test]
fn runtime_existing_remote_proposal_promotion_commits_while_store_read_is_held() {
    const INGRESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    let runtime = runtime();
    let consensus = ConsensusBuilder::default().build();
    let transaction = TransactionBuilder::default()
        .input(ckb_types::packed::CellInput::new(
            ckb_types::packed::OutPoint::new(ckb_types::packed::Byte32::new([0x74; 32]), 0),
            0,
        ))
        .output(ckb_types::packed::CellOutput::default())
        .output_data(ckb_types::bytes::Bytes::new().pack())
        .build();
    let key = RawTxHash(transaction.hash());
    let remote = remote_at_for_foundation(
        transaction.clone(),
        0,
        PeerIndex::from(0x74usize),
        100,
        &consensus,
    )
    .map(RetainedIngressAttempt::Validated)
    .unwrap_or_else(|attempt| attempt);
    let remote_batch = RetainedAdmissionBatch::new(remote, std::collections::VecDeque::new())
        .expect("one remote attempt is a homogeneous batch");
    let Ok((consumed, remaining, fault)) = runtime.commit_retained_ingress_batch(remote_batch)
    else {
        panic!("the initial Remote owner commits");
    };
    assert_eq!(consumed, 1);
    assert!(remaining.is_empty());
    assert_eq!(fault, None);
    let remote_version = runtime
        .store
        .read()
        .authority
        .entry(&key)
        .expect("the Remote owner exists")
        .record()
        .version;

    let bounded = BoundedTransaction::try_new(transaction)
        .expect("the proposal promotion fixture transaction is bounded");
    let proposal = proposal(bounded, &consensus);
    let proposal_batch = RetainedAdmissionBatch::new(proposal, std::collections::VecDeque::new())
        .expect("one proposal attempt is a homogeneous batch");

    let held_read = runtime.store.read();
    let worker_runtime = runtime.clone();
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        result_tx
            .send(worker_runtime.commit_retained_ingress_batch(proposal_batch))
            .expect("the retained-ingress observer remains alive");
    });
    let result = result_rx.recv_timeout(INGRESS_TIMEOUT);
    drop(held_read);
    worker
        .join()
        .expect("the proposal promotion worker remains healthy");
    let (consumed, remaining, fault) = result
        .expect("existing-owner Proposal promotion cannot require the outer write guard")
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the existing Remote owner promotes through the shared route")
        });
    assert_eq!(consumed, 1);
    assert!(remaining.is_empty());
    assert_eq!(fault, None);
    assert!(matches!(
        runtime.store.read().authority.entry(&key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Proposal { .. })
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
                && entry.record.version != remote_version
    ));
}

fn retained_promotion_transaction(marker: u8) -> ckb_types::core::TransactionView {
    TransactionBuilder::default()
        .input(ckb_types::packed::CellInput::new(
            ckb_types::packed::OutPoint::new(ckb_types::packed::Byte32::new([marker; 32]), 0),
            0,
        ))
        .output(ckb_types::packed::CellOutput::default())
        .output_data(ckb_types::bytes::Bytes::new().pack())
        .build()
}

fn retained_proposal_batch(
    marker: u8,
    consensus: &ckb_chain_spec::consensus::Consensus,
) -> RetainedAdmissionBatch {
    let bounded = BoundedTransaction::try_new(retained_promotion_transaction(marker))
        .expect("the proposal fixture transaction is bounded");
    let attempt = proposal(bounded, consensus);
    assert!(matches!(
        &attempt,
        crate::authority::ingress::RetainedIngressAttempt::Validated(_)
    ));
    RetainedAdmissionBatch::new(attempt, std::collections::VecDeque::new())
        .expect("one proposal attempt is a homogeneous batch")
}

fn retained_remote_batch(
    peer: PeerIndex,
    transactions: impl IntoIterator<Item = ckb_types::core::TransactionView>,
    consensus: &ckb_chain_spec::consensus::Consensus,
) -> RetainedAdmissionBatch {
    let mut attempts = transactions
        .into_iter()
        .map(|transaction| {
            remote_at_for_foundation(transaction, 0, peer, 100, consensus)
                .map(RetainedIngressAttempt::Validated)
                .unwrap_or_else(|attempt| attempt)
        })
        .collect::<std::collections::VecDeque<_>>();
    let head = attempts
        .pop_front()
        .expect("the fixture constructs a nonempty Remote batch");
    RetainedAdmissionBatch::new(head, attempts).expect("the Remote batch is homogeneous")
}

fn malformed_remote_batch(
    peer: PeerIndex,
    transaction: ckb_types::core::TransactionView,
    consensus: &ckb_chain_spec::consensus::Consensus,
) -> RetainedAdmissionBatch {
    let attempt = remote_at_for_foundation(
        transaction,
        consensus
            .max_block_cycles()
            .checked_add(1)
            .expect("the fixture consensus cycle bound is representable"),
        peer,
        100,
        consensus,
    )
    .map(RetainedIngressAttempt::Validated)
    .unwrap_or_else(|attempt| attempt);
    assert!(matches!(attempt, RetainedIngressAttempt::Rejected(_)));
    RetainedAdmissionBatch::new(attempt, std::collections::VecDeque::new())
        .expect("one malformed Remote attempt is a homogeneous batch")
}

#[test]
fn runtime_disjoint_shared_promotions_compile_and_commit_while_first_cut_is_live() {
    const CUT_ENTRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let consensus = ConsensusBuilder::default().build();
    for marker in 0u8..60 {
        runtime
            .admit(
                ValidatedAdmission::remote(
                    retained_promotion_transaction(marker),
                    PeerIndex::from(usize::from(marker)),
                )
                .expect("the candidate Remote admission is valid"),
            )
            .expect("the candidate Remote owner commits");
    }

    let (left_marker, right_marker) = runtime.with_authority_read_for_foundation(|authority| {
        let mut selected = None;
        'left: for left_marker in 0u8..60 {
            let Some(left) = authority
                .compile_shared_retained_ingress_batch(&retained_proposal_batch(
                    left_marker,
                    &consensus,
                ))
                .expect("the left promotion compiles")
            else {
                continue;
            };
            let left_support = left.physical_write_support_for_foundation(authority);
            for right_marker in (left_marker + 1)..60 {
                let Some(right) = authority
                    .compile_shared_retained_ingress_batch(&retained_proposal_batch(
                        right_marker,
                        &consensus,
                    ))
                    .expect("the right promotion compiles")
                else {
                    continue;
                };
                if left_support.is_disjoint(right.physical_write_support_for_foundation(authority))
                {
                    selected = Some((left_marker, right_marker));
                    break 'left;
                }
            }
        }
        selected.expect("the fixed layout contains two disjoint promotion cuts")
    });

    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::FinalCutBeforeActivation,
            Some(probe),
        );
    });
    let committed = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let consensus_ref = &consensus;
        let left = scope.spawn(move || {
            runtime_ref
                .commit_retained_ingress_batch(retained_proposal_batch(left_marker, consensus_ref))
                .map_err(drop)
        });
        entered
            .recv_timeout(CUT_ENTRY_TIMEOUT)
            .expect("the first promotion reaches its final owner cut");
        let runtime_ref = &runtime;
        let consensus_ref = &consensus;
        let right = scope.spawn(move || {
            runtime_ref
                .commit_retained_ingress_batch(retained_proposal_batch(right_marker, consensus_ref))
                .map_err(drop)
        });
        let overlapped = entered.recv_timeout(CUT_ENTRY_TIMEOUT).is_ok();
        release.send(()).expect("release the first live cut");
        if !overlapped {
            entered
                .recv_timeout(CUT_ENTRY_TIMEOUT)
                .expect("the second promotion reaches its cut after the first releases");
        }
        release.send(()).expect("release the second live cut");
        let results = [
            left.join().expect("the left promotion thread joins"),
            right.join().expect("the right promotion thread joins"),
        ];
        assert!(
            overlapped,
            "a disjoint promotion must compile and enter its cut while the first cut is live"
        );
        results
    });
    for result in committed {
        let (consumed, remaining, fault) =
            result.unwrap_or_else(|()| panic!("each disjoint promotion commits"));
        assert_eq!(consumed, 1);
        assert!(remaining.is_empty());
        assert_eq!(fault, None);
    }
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::FinalCutBeforeActivation, None);
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn runtime_effect_only_prefix_commits_under_read_and_returns_the_exact_owner_suffix() {
    const INGRESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let runtime = runtime();
    let consensus = ConsensusBuilder::default().build();
    let peer = PeerIndex::from(181usize);
    let invalid = retained_promotion_transaction(181)
        .as_advanced_builder()
        .version(1u32)
        .build();
    let owner = retained_promotion_transaction(182);
    let owner_key = RawTxHash(owner.hash());
    let batch = retained_remote_batch(peer, [invalid, owner], &consensus);
    let before_clocks = runtime.store.read().authority.clocks();

    let held_read = runtime.store.read();
    let worker_runtime = runtime.clone();
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        result_tx
            .send(worker_runtime.commit_retained_ingress_batch(batch))
            .expect("the retained-prefix observer remains alive");
    });
    let result = result_rx.recv_timeout(INGRESS_TIMEOUT);
    drop(held_read);
    worker
        .join()
        .expect("the retained-prefix worker remains healthy");
    let (consumed, mut remaining, fault) = result
        .expect("an effect-only prefix cannot require the outer write guard")
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the effect-only prefix commits through the sole staged EffectLog")
        });
    assert_eq!(consumed, 1);
    assert_eq!(remaining.len(), 1);
    assert_eq!(fault, None);
    assert!(runtime.store.read().authority.entry(&owner_key).is_none());
    assert_eq!(runtime.effect_observation_for_foundation().queued.len(), 1);
    let prefix_clocks = runtime.store.read().authority.clocks();
    assert_eq!(prefix_clocks.next_version, before_clocks.next_version);
    assert_eq!(prefix_clocks.next_arrival, before_clocks.next_arrival);

    let head = remaining
        .pop_front()
        .expect("the exact owner-producing suffix remains");
    let suffix = RetainedAdmissionBatch::new(head, remaining)
        .expect("the returned suffix preserves its Remote batch identity");
    let (consumed, remaining, fault) = runtime
        .commit_retained_ingress_batch(suffix)
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the returned owner suffix commits in the next canonical round")
        });
    assert_eq!(consumed, 1);
    assert!(remaining.is_empty());
    assert_eq!(fault, None);
    assert!(runtime.store.read().authority.entry(&owner_key).is_some());
    let committed_clocks = runtime.store.read().authority.clocks();
    assert_eq!(
        committed_clocks.next_version.0,
        before_clocks.next_version.0 + 1
    );
    assert_eq!(
        committed_clocks.next_arrival.0,
        before_clocks.next_arrival.0 + 1
    );
}

#[test]
fn runtime_owner_then_effect_batch_completes_as_two_shared_prefixes() {
    const INGRESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let runtime = runtime();
    let consensus = ConsensusBuilder::default().build();
    let peer = PeerIndex::from(183usize);
    let transaction = retained_promotion_transaction(183);
    let key = RawTxHash(transaction.hash());
    let batch = retained_remote_batch(peer, [transaction.clone(), transaction], &consensus);

    let held_read = runtime.store.read();
    let worker_runtime = runtime.clone();
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let (first_consumed, mut remaining, first_fault) = worker_runtime
            .commit_retained_ingress_batch(batch)
            .unwrap_or_else(|failure| {
                drop(failure);
                panic!("the owner prefix commits through the shared owner route")
            });
        let head = remaining
            .pop_front()
            .expect("the effect suffix remains after the owner prefix");
        let suffix = RetainedAdmissionBatch::new(head, remaining)
            .expect("the effect suffix preserves the Remote batch identity");
        let (second_consumed, remaining, second_fault) = worker_runtime
            .commit_retained_ingress_batch(suffix)
            .unwrap_or_else(|failure| {
                drop(failure);
                panic!("the effect suffix commits through the shared effect route")
            });
        result_tx
            .send((
                first_consumed,
                first_fault,
                second_consumed,
                remaining.len(),
                second_fault,
            ))
            .expect("the mixed-prefix observer remains alive");
    });
    let result = result_rx.recv_timeout(INGRESS_TIMEOUT);
    drop(held_read);
    worker
        .join()
        .expect("the mixed-prefix worker remains healthy");
    let (first_consumed, first_fault, second_consumed, remaining, second_fault) =
        result.expect("owner and effect prefixes must both complete without the outer write guard");
    assert_eq!((first_consumed, first_fault), (1, None));
    assert_eq!((second_consumed, remaining, second_fault), (1, 0, None));
    assert!(runtime.store.read().authority.entry(&key).is_some());
    assert_eq!(runtime.effect_observation_for_foundation().queued.len(), 1);
}

#[test]
fn runtime_effect_prefix_does_not_lock_its_unconsumed_owner_suffix() {
    const CUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let consensus = ConsensusBuilder::default().build();
    let peer = PeerIndex::from(184usize);
    let invalid = retained_promotion_transaction(184)
        .as_advanced_builder()
        .version(1u32)
        .build();
    let owner = retained_promotion_transaction(185);
    let owner_key = RawTxHash(owner.hash());
    let batch = retained_remote_batch(peer, [invalid, owner.clone()], &consensus);

    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::EffectReadCutBeforeActivation,
            Some(probe),
        );
    });
    let left_runtime = runtime.clone();
    let left = std::thread::spawn(move || {
        left_runtime
            .commit_retained_ingress_batch(batch)
            .map_err(drop)
    });
    entered
        .recv_timeout(CUT_TIMEOUT)
        .expect("the effect prefix holds only its consumed support");

    let right_runtime = runtime.clone();
    let right_batch = retained_remote_batch(peer, [owner], &consensus);
    let (right_tx, right_rx) = std::sync::mpsc::sync_channel(1);
    let right = std::thread::spawn(move || {
        right_tx
            .send(right_runtime.commit_retained_ingress_batch(right_batch))
            .expect("the suffix-writer observer remains alive");
    });
    let right_result = right_rx.recv_timeout(CUT_TIMEOUT);
    release.send(()).expect("release the effect prefix");
    let left_result = left.join().expect("the effect-prefix worker joins");
    right.join().expect("the suffix-writer worker joins");

    let (consumed, remaining, fault) = right_result
        .expect("an unconsumed suffix writer must not be blocked by the effect read cut")
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the unconsumed owner commits through its own shared cut")
        });
    assert_eq!(consumed, 1);
    assert!(remaining.is_empty());
    assert_eq!(fault, None);
    let (consumed, remaining, fault) = left_result
        .unwrap_or_else(|()| panic!("the effect prefix commits after its suffix writer"));
    assert_eq!(consumed, 1);
    assert_eq!(remaining.len(), 1);
    assert_eq!(fault, None);
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::EffectReadCutBeforeActivation, None);
        assert!(authority.entry(&owner_key).is_some());
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn runtime_disjoint_effect_only_prefixes_overlap_inside_routed_read_cuts() {
    const CUT_ENTRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let consensus = ConsensusBuilder::default().build();
    let invalid = |marker| {
        retained_promotion_transaction(marker)
            .as_advanced_builder()
            .version(1u32)
            .build()
    };

    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::EffectReadCutBeforeActivation,
            Some(probe),
        );
    });
    let results = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let consensus_ref = &consensus;
        let left = scope.spawn(move || {
            runtime_ref
                .commit_retained_ingress_batch(retained_remote_batch(
                    PeerIndex::from(191usize),
                    [invalid(191)],
                    consensus_ref,
                ))
                .map_err(drop)
        });
        entered
            .recv_timeout(CUT_ENTRY_TIMEOUT)
            .expect("the first effect prefix holds its routed read cut");
        let runtime_ref = &runtime;
        let consensus_ref = &consensus;
        let right = scope.spawn(move || {
            runtime_ref
                .commit_retained_ingress_batch(retained_remote_batch(
                    PeerIndex::from(192usize),
                    [invalid(192)],
                    consensus_ref,
                ))
                .map_err(drop)
        });
        entered.recv_timeout(CUT_ENTRY_TIMEOUT).expect(
            "the second effect prefix reaches activation while the first routed cut is live",
        );
        release.send(()).expect("release the first effect prefix");
        release.send(()).expect("release the second effect prefix");
        [
            left.join().expect("the left effect worker joins"),
            right.join().expect("the right effect worker joins"),
        ]
    });
    for result in results {
        let (consumed, remaining, fault) =
            result.unwrap_or_else(|()| panic!("each effect-only prefix commits"));
        assert_eq!(consumed, 1);
        assert!(remaining.is_empty());
        assert_eq!(fault, None);
    }
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::EffectReadCutBeforeActivation, None);
        assert!(authority.primary_projection_consistent());
    });
    assert_eq!(runtime.effect_observation_for_foundation().queued.len(), 2);
}

#[test]
fn runtime_malformed_effect_capacity_failure_rolls_back_slot_before_the_peer_fence() {
    let runtime = runtime_with_one_effect_batch();
    let consensus = ConsensusBuilder::default().build();
    let peer = PeerIndex::from(229usize);
    queue_rejection(&runtime, EffectPolicy::Remote, 227);
    queue_rejection(&runtime, EffectPolicy::Trusted, 228);
    queue_rejection(&runtime, EffectPolicy::CriticalDetail, 229);
    let before =
        runtime.with_authority_read_for_foundation(|authority| authority.normalized_snapshot());

    let failure = runtime
        .commit_retained_ingress_batch(malformed_remote_batch(
            peer,
            retained_promotion_transaction(229),
            &consensus,
        ))
        .expect_err("a full critical region rejects the staged cohort effect");
    let (reason, _batch) = failure.into_parts();
    assert!(matches!(
        reason,
        RetainedIngressBatchFailureReason::Plan(PlanError::Backpressure(
            Backpressure::EffectCapacity
        ))
    ));
    runtime.with_authority_read_for_foundation(|authority| {
        let after = authority.normalized_snapshot();
        assert!(after.equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1));
        assert!(!authority.peer_is_banned_for_reference(peer));
        assert!(
            authority
                .preaccepted_for_peer_for_reference(peer)
                .is_empty()
        );
        assert!(authority.primary_projection_consistent());
    });

    let admitted = runtime
        .commit_retained_ingress_batch(retained_remote_batch(
            peer,
            [retained_promotion_transaction(228)],
            &consensus,
        ))
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the rolled-back slot leaves ordinary same-peer ingress available")
        });
    assert_eq!(admitted.0, 1);
}

#[test]
fn runtime_hidden_peer_revocation_blocks_only_the_same_peer() {
    const ENTRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let consensus = ConsensusBuilder::default().build();
    let blocked_peer = PeerIndex::from(230usize);
    let independent_peer = PeerIndex::from(231usize);
    let existing = retained_promotion_transaction(233);
    let existing_key = RawTxHash(existing.hash());
    let admitted = runtime
        .commit_retained_ingress_batch(retained_remote_batch(blocked_peer, [existing], &consensus))
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the same-peer promotion fixture first owns a Remote row")
        });
    assert_eq!(admitted.0, 1);
    let stale_promotion = runtime.with_authority_read_for_foundation(|authority| {
        authority
            .compile_shared_retained_ingress_batch(&retained_proposal_batch(233, &consensus))
            .expect("the same-peer Proposal promotion compiles")
            .expect("the existing Remote owner has a shared promotion shape")
    });
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::PeerFenceHiddenBeforeCohort,
            Some(probe),
        );
    });
    let held_outer_read = runtime.store.read();
    let blocked = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let consensus_ref = &consensus;
        let first = scope.spawn(move || {
            runtime_ref
                .commit_retained_ingress_batch(malformed_remote_batch(
                    blocked_peer,
                    retained_promotion_transaction(230),
                    consensus_ref,
                ))
                .map_err(drop)
        });
        entered
            .recv_timeout(ENTRY_TIMEOUT)
            .expect("the peer-local hidden fence is installed");

        let independent = runtime
            .commit_retained_ingress_batch(retained_remote_batch(
                independent_peer,
                [retained_promotion_transaction(231)],
                &consensus,
            ))
            .unwrap_or_else(|failure| {
                drop(failure);
                panic!("an unrelated peer commits while the hidden fence is live")
            });
        assert_eq!(independent.0, 1);
        assert!(independent.1.is_empty());

        let same_peer = runtime
            .commit_retained_ingress_batch(retained_remote_batch(
                blocked_peer,
                [retained_promotion_transaction(232)],
                &consensus,
            ))
            .expect_err("the same peer cannot grow a hidden revocation cohort");
        let (reason, _batch) = same_peer.into_parts();
        assert!(matches!(
            reason,
            RetainedIngressBatchFailureReason::SharedContention
        ));
        let stale = runtime.with_authority_read_for_foundation(|authority| {
            stale_promotion
                .bind(authority)
                .expect("the generation remains current")
                .apply()
        });
        assert!(matches!(
            stale,
            Err(super::ConcurrentRetainedIngressError::Stale)
        ));
        runtime.with_authority_read_for_foundation(|authority| {
            assert!(matches!(
                authority.entry(&existing_key),
                Some(OwnedTx::PreAccepted(entry))
                    if matches!(entry.source, PreAcceptedSource::Remote(_))
            ));
        });
        release.send(()).expect("release the hidden peer fence");
        first.join().expect("the revocation worker joins")
    });
    drop(held_outer_read);
    let (consumed, remaining, fault) =
        blocked.unwrap_or_else(|()| panic!("the malformed revocation commits"));
    assert_eq!(consumed, 1);
    assert!(remaining.is_empty());
    assert_eq!(fault, None);
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::PeerFenceHiddenBeforeCohort, None);
        assert!(authority.peer_is_banned_for_reference(blocked_peer));
        assert!(authority.entry(&existing_key).is_none());
        assert!(
            authority
                .entry(&RawTxHash(retained_promotion_transaction(231).hash()))
                .is_some()
        );
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test]
async fn runtime_generation_replacement_yields_to_a_hidden_peer_fence_and_preserves_it() {
    const ENTRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let consensus = ConsensusBuilder::default().build();
    let banned_peer = PeerIndex::from(233usize);
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::PeerFenceHiddenBeforeCohort,
            Some(probe),
        );
    });
    let blocked = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let consensus_ref = &consensus;
        let first = scope.spawn(move || {
            runtime_ref
                .commit_retained_ingress_batch(malformed_remote_batch(
                    banned_peer,
                    retained_promotion_transaction(233),
                    consensus_ref,
                ))
                .map_err(drop)
        });
        entered
            .recv_timeout(ENTRY_TIMEOUT)
            .expect("the hidden peer fence is installed before its cohort cut");
        assert!(
            !runtime.generation_write_available_for_foundation(),
            "generation replacement cannot acquire its write cut while a hidden fence session is live"
        );
        release.send(()).expect("release the hidden peer fence");
        first.join().expect("the revocation worker joins")
    });
    let (consumed, remaining, fault) =
        blocked.unwrap_or_else(|()| panic!("the malformed revocation commits"));
    assert_eq!(consumed, 1);
    assert!(remaining.is_empty());
    assert_eq!(fault, None);
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_ingress_probe(SharedIngressProbePhase::PeerFenceHiddenBeforeCohort, None);
    });
    let (row_before, swaps_before) = runtime.with_authority_read_for_foundation(|authority| {
        let entries = authority.entries_for_reference();
        (
            entries.peer_ingress_row(banned_peer),
            entries.generation_payload_swaps_for_test(),
        )
    });
    runtime
        .clear_pool(genesis_snapshot())
        .await
        .expect("generation replacement follows the committed revocation");
    runtime.with_authority_read_for_foundation(|authority| {
        let entries = authority.entries_for_reference();
        assert_eq!(
            entries.peer_ingress_row(banned_peer),
            row_before,
            "the exact active fence row rides the persistent envelope across replacement"
        );
        assert!(authority.peer_is_banned_for_reference(banned_peer));
        assert!(!authority.peer_fence_hidden_for_reference(banned_peer));
        assert_eq!(
            entries.generation_payload_swaps_for_test(),
            swaps_before + crate::authority::shard::AUTHORITY_SHARD_COUNT
        );
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn runtime_disjoint_malformed_peer_revocations_overlap_under_outer_read() {
    const CUT_ENTRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let consensus = ConsensusBuilder::default().build();
    let (left_peer, right_peer, right_support) =
        runtime.with_authority_read_for_foundation(|authority| {
            let mut selected = None;
            'left: for left in (240usize..320).map(PeerIndex::from) {
                let left_plan = authority
                    .plan_shared_peer_revocation(&malformed_remote_batch(
                        left,
                        retained_promotion_transaction(240),
                        &consensus,
                    ))
                    .expect("the left malformed shape plans")
                    .expect("the left malformed shape is selected");
                let left_support = left_plan.physical_write_support_for_foundation();
                drop(left_plan);
                for right in (320usize..520).map(PeerIndex::from) {
                    let right_plan = authority
                        .plan_shared_peer_revocation(&malformed_remote_batch(
                            right,
                            retained_promotion_transaction(241),
                            &consensus,
                        ))
                        .expect("the right malformed shape plans")
                        .expect("the right malformed shape is selected");
                    let right_support = right_plan.physical_write_support_for_foundation();
                    drop(right_plan);
                    if left_support.is_disjoint(right_support) {
                        selected = Some((left, right, right_support));
                        break 'left;
                    }
                }
            }
            selected.expect("the fixed layout contains two disjoint malformed peer cuts")
        });
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_owner_commit_probe(Some(probe));
    });
    let held_outer_read = runtime.store.read();
    let committed = std::thread::scope(|scope| {
        let runtime_ref = &runtime;
        let consensus_ref = &consensus;
        let left = scope.spawn(move || {
            runtime_ref
                .commit_retained_ingress_batch(malformed_remote_batch(
                    left_peer,
                    retained_promotion_transaction(240),
                    consensus_ref,
                ))
                .map_err(drop)
        });
        entered
            .recv_timeout(CUT_ENTRY_TIMEOUT)
            .expect("the first malformed revocation reached its real final cut");
        runtime.with_authority_read_for_foundation(|authority| {
            assert!(
                authority
                    .entries_for_reference()
                    .try_write_cut(right_support)
                    .is_some(),
                "the selected second physical cut remains available while the first is live"
            );
        });
        let runtime_ref = &runtime;
        let consensus_ref = &consensus;
        let right = scope.spawn(move || {
            runtime_ref
                .commit_retained_ingress_batch(malformed_remote_batch(
                    right_peer,
                    retained_promotion_transaction(241),
                    consensus_ref,
                ))
                .map_err(drop)
        });
        let overlapped = entered.recv_timeout(CUT_ENTRY_TIMEOUT).is_ok();
        release.send(()).expect("release one malformed final cut");
        if !overlapped {
            entered
                .recv_timeout(CUT_ENTRY_TIMEOUT)
                .expect("the second malformed revocation eventually reaches its cut");
        }
        release
            .send(())
            .expect("release the other malformed final cut");
        assert!(overlapped, "disjoint peer revocations must overlap");
        [
            left.join().expect("left revocation joins"),
            right.join().expect("right revocation joins"),
        ]
    });
    drop(held_outer_read);
    for result in committed {
        let (consumed, remaining, fault) =
            result.unwrap_or_else(|()| panic!("each peer revocation commits"));
        assert_eq!(consumed, 1);
        assert!(remaining.is_empty());
        assert_eq!(fault, None);
    }
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_owner_commit_probe(None);
        assert!(authority.peer_is_banned_for_reference(left_peer));
        assert!(authority.peer_is_banned_for_reference(right_peer));
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn runtime_malformed_same_raw_never_removes_another_peer_owner() {
    let runtime = runtime();
    let consensus = ConsensusBuilder::default().build();
    let owner_peer = PeerIndex::from(250usize);
    let culprit_peer = PeerIndex::from(251usize);
    let transaction = retained_promotion_transaction(250);
    let key = RawTxHash(transaction.hash());
    let admitted = runtime
        .commit_retained_ingress_batch(retained_remote_batch(
            owner_peer,
            [transaction.clone()],
            &consensus,
        ))
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the other peer owns the valid raw transaction")
        });
    assert_eq!(admitted.0, 1);

    let revoked = runtime
        .commit_retained_ingress_batch(malformed_remote_batch(
            culprit_peer,
            transaction,
            &consensus,
        ))
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the malformed evidence revokes only its own empty cohort")
        });
    assert_eq!(revoked.0, 1);
    runtime.with_authority_read_for_foundation(|authority| {
        assert!(authority.entry(&key).is_some());
        assert!(authority.peer_is_banned_for_reference(culprit_peer));
        assert!(!authority.peer_is_banned_for_reference(owner_peer));
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn runtime_mixed_local_and_malformed_batch_never_commits_a_shared_prefix() {
    const OUTER_READER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let runtime = runtime_with_one_accepted_owner();
    let peer = 108usize;
    let local = admission(1_068, peer);
    let local_key = local.identity.raw.clone();
    runtime.admit(local).expect("the local fixture commits");
    let culprit = admission(1_069, peer);
    let culprit_key = culprit.identity.raw.clone();
    runtime
        .admit(culprit)
        .expect("the malformed fixture commits");

    let completions = {
        let mut store = runtime.store.write();
        let local_version = store
            .authority
            .entry(&local_key)
            .expect("the local owner exists")
            .record()
            .version;
        let local_checkout = store
            .authority
            .plan_checkout_for_foundation(&local_key, local_version, WorkPermit::ResolveOnly)
            .expect("the local fixture checks out")
            .apply();
        let CheckedOutWork::Resolve(local_work) = local_checkout.into_work() else {
            panic!("the local resolve-only fixture returns Resolve work")
        };
        let culprit_version = store
            .authority
            .entry(&culprit_key)
            .expect("the culprit owner exists")
            .record()
            .version;
        let culprit_checkout = store
            .authority
            .plan_checkout_for_foundation(&culprit_key, culprit_version, WorkPermit::ResolveOnly)
            .expect("the culprit fixture checks out")
            .apply();
        let CheckedOutWork::Resolve(culprit_work) = culprit_checkout.into_work() else {
            panic!("the culprit resolve-only fixture returns Resolve work")
        };
        vec![
            ComputeExchangeCompletion::new(
                ComputeWorkerSlot::ordered_resolve(),
                local_work.internal_failure(),
            ),
            ComputeExchangeCompletion::new(
                ComputeWorkerSlot::from(ComputeVerifierSlot::new(0, VerifyCapability::Any)),
                culprit_work.rejected(Reject::Malformed(
                    "runtime-cohort".to_owned(),
                    "malformed completion must dominate its peer cohort".to_owned(),
                )),
            ),
        ]
    };

    let held_exchange_read = runtime.store.read();
    let exchange_runtime = runtime.clone();
    let (exchange_tx, exchange_rx) = std::sync::mpsc::sync_channel(1);
    let exchange_worker = std::thread::spawn(move || {
        exchange_tx
            .send(exchange_runtime.exchange_compute(completions, Vec::new()))
            .expect("the malformed exchange observer remains alive");
    });
    let exchange_before_release = exchange_rx.recv_timeout(OUTER_READER_TIMEOUT);
    let exchange_finished_while_reader_held = exchange_before_release.is_ok();
    drop(held_exchange_read);
    let committed = match exchange_before_release {
        Ok(committed) => committed,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => exchange_rx
            .recv()
            .expect("the predecessor exchange finishes after reader release"),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("the malformed exchange worker remains connected")
        }
    }
    .unwrap_or_else(|failure| {
        drop(failure);
        panic!("the canonical malformed-peer transition commits")
    });
    exchange_worker
        .join()
        .expect("the malformed exchange worker remains healthy");
    assert!(
        exchange_finished_while_reader_held,
        "malformed nonlocal exchange still requires the outer AuthorityStore writer"
    );
    assert!(committed.settled.is_empty());
    assert_eq!(committed.deferred.len(), 2);
    let mut exact = None;
    let mut after_effect = None;
    for deferred in committed.deferred {
        let (route, completion) = deferred.into_parts();
        match route {
            ComputeExchangeDeferredRoute::ExactSettlement => exact = Some(completion),
            ComputeExchangeDeferredRoute::ExchangeAfterEffect => after_effect = Some(completion),
            ComputeExchangeDeferredRoute::ExchangeRetry => {
                panic!("same-peer work waits for the revocation effect")
            }
        }
    }
    let (culprit_slot, culprit_finished) = exact
        .expect("the malformed completion keeps exact precedence")
        .into_parts();
    assert_eq!(
        culprit_slot,
        ComputeWorkerSlot::from(ComputeVerifierSlot::new(0, VerifyCapability::Any))
    );
    let held_revocation_read = runtime.store.read();
    let revocation_runtime = runtime.clone();
    let (revocation_tx, revocation_rx) = std::sync::mpsc::sync_channel(1);
    let revocation_worker = std::thread::spawn(move || {
        revocation_tx
            .send(revocation_runtime.settle_finished(culprit_finished))
            .expect("the malformed exact-settlement observer remains alive");
    });
    let revocation_before_release = revocation_rx.recv_timeout(OUTER_READER_TIMEOUT);
    let revocation_finished_while_reader_held = revocation_before_release.is_ok();
    drop(held_revocation_read);
    let committed_revocation = match revocation_before_release {
        Ok(committed) => committed,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => revocation_rx
            .recv()
            .expect("the predecessor revocation finishes after reader release"),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("the malformed exact-settlement worker remains connected")
        }
    };
    revocation_worker
        .join()
        .expect("the malformed exact-settlement worker remains healthy");
    assert!(
        revocation_finished_while_reader_held,
        "malformed peer revocation still requires the outer AuthorityStore writer"
    );
    let committed_revocation = continued(committed_revocation);
    let (aftermath, post_commit_fault) = committed_revocation.into_parts();
    assert_eq!(post_commit_fault, None);
    drop(aftermath);
    assert!(
        runtime.store.read().authority.entry(&local_key).is_none(),
        "an earlier owner-local completion cannot publish before later peer revocation"
    );
    assert!(runtime.store.read().authority.entry(&culprit_key).is_none());
    runtime.with_authority_read_for_foundation(|authority| {
        assert!(authority.peer_is_banned_for_reference(PeerIndex::from(peer)));
    });
    let after_effect = after_effect.expect("the earlier same-peer result remains linear");
    let obsolete = runtime
        .exchange_compute(vec![after_effect], Vec::new())
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the revoked same-peer completion becomes obsolete")
        });
    assert_eq!(
        obsolete.obsolete,
        vec![ComputeWorkerSlot::ordered_resolve()]
    );
}

#[tokio::test]
async fn runtime_compute_wake_coalesces_role_heads_without_becoming_authority() {
    let runtime = runtime();

    runtime
        .admit(admission(1_064, 99))
        .expect("the small-cycle admission commits");
    expect_signal(
        runtime.compute_signal(),
        "admission must publish the shared compute level",
    )
    .await;
    expect_no_signal(
        runtime.compute_signal(),
        "one owner transition must not duplicate the coalesced compute hint",
    )
    .await;

    let small = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveOnly)
            .expect("small-cycle resolution checkout remains healthy"),
    )
    .expect("small-cycle resolution is ready");
    let small = completion(runtime.execute_compute(small));
    drop(continued(runtime.settle_completion(small)));
    expect_signal(
        runtime.compute_signal(),
        "a queued Verify head must publish the shared compute level",
    )
    .await;
    expect_no_signal(
        runtime.compute_signal(),
        "Small and Any projections must still coalesce to one level hint",
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
        runtime.compute_signal(),
        "the second admission must republish the compute level",
    )
    .await;
    let large = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveOnly)
            .expect("large-cycle resolution checkout remains healthy"),
    )
    .expect("large-cycle resolution is ready");
    let large = completion(runtime.execute_compute(large));
    drop(continued(runtime.settle_completion(large)));
    expect_signal(
        runtime.compute_signal(),
        "a distinct large head and active-work release publish one level",
    )
    .await;
    expect_no_signal(
        runtime.compute_signal(),
        "one Apply cannot publish separate role-routing decisions",
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
    let AuthorityComputeOutcome::Verification(request) = runtime.execute_compute(job) else {
        panic!("the empty-script fixture fits continuous verification")
    };
    let cache = ckb_verification::cache::init_cache();
    let (_command_tx, mut command_rx) =
        tokio::sync::watch::channel(ckb_script::ChunkCommand::Resume);
    let completion = runtime
        .execute_verification(request.bind_cache(&cache), &mut command_rx)
        .await;
    let store = runtime.store.read();
    assert!(matches!(
        store.authority.entry(&key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    drop(store);
    let verification = continued(runtime.settle_completion(completion));
    assert!(verification.into_parts().1.is_some());
    assert!(matches!(
        runtime
            .try_drive_ready()
            .expect("the sealed Ready batch commits"),
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Applied)
    ));
    let store = runtime.store.read();
    assert!(matches!(
        store.authority.entry(&key),
        Some(OwnedTx::Accepted(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_ready_wave_compiles_and_dispatches_while_an_outer_reader_is_held() {
    const DISPATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let runtime = runtime();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded Ready compilation fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_080 + offset as u32, 1_080 + offset).await);
    }
    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);

    let dispatch = drive_ready_while_outer_reader_is_held(&runtime, DISPATCH_TIMEOUT)
        .expect("the compatible Ready pair compiles while the reader is held");

    let AuthorityReadyDispatch::Wave(wave) = dispatch else {
        panic!("the compatible Ready pair must dispatch one shared wave");
    };
    let (assignments, wave_ends) = wave.into_parts();
    assert_eq!(assignments.len(), selected.len());
    assert_eq!(wave_ends, vec![selected.len()]);
    for (index, assignment) in assignments.into_iter().enumerate() {
        assert!(matches!(
            runtime
                .commit_ready_assignment(AuthorityReadyCommitLane::from_index(index), assignment,),
            AuthorityReadyCommitTerminal::Applied
        ));
    }
    runtime.with_authority_for_foundation(|authority| {
        assert!(
            selected
                .iter()
                .all(|hash| matches!(authority.entry(hash), Some(OwnedTx::Accepted(_))))
        );
        assert!(authority.primary_projection_consistent());
    });
    drop(excluded_reservation);
    runtime.with_authority_for_foundation(|authority| {
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_ready_relation_classification_releases_owner_before_secondary_shard_read() {
    let runtime = runtime();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded lock-order fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_180 + offset as u32, 1_180 + offset).await);
    }
    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);

    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_read_for_foundation(|authority| {
        authority.set_membership_secondary_read_probe_for_foundation(selected[0].clone(), probe);
    });
    let driver_runtime = runtime.clone();
    let driver = std::thread::spawn(move || driver_runtime.try_drive_ready());
    entered
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("the Ready classifier reaches its secondary shard observation");
    let owner_write_available = runtime.with_authority_read_for_foundation(|authority| {
        authority.owner_shard_write_available_for_foundation(&selected[0])
    });
    release
        .send(())
        .expect("the Ready classifier resumes after the lock-order observation");
    let dispatch = driver
        .join()
        .expect("the Ready classifier thread remains healthy")
        .expect("the compatible Ready pair compiles");
    assert!(
        owner_write_available,
        "the owner point read must end before a differently routed membership read begins"
    );

    let AuthorityReadyDispatch::Wave(wave) = dispatch else {
        panic!("the compatible Ready pair must dispatch one shared wave");
    };
    let (assignments, wave_ends) = wave.into_parts();
    assert_eq!(assignments.len(), selected.len());
    assert_eq!(wave_ends, vec![selected.len()]);
    for (index, assignment) in assignments.into_iter().enumerate() {
        assert!(matches!(
            runtime
                .commit_ready_assignment(AuthorityReadyCommitLane::from_index(index), assignment,),
            AuthorityReadyCommitTerminal::Applied
        ));
    }
    drop(excluded_reservation);
    runtime.with_authority_read_for_foundation(|authority| {
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_single_ready_compiles_and_commits_while_an_outer_reader_is_held() {
    const DISPATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let runtime = runtime();
    let hash = advance_remote_to_ready(&runtime, 1_089, 1_089).await;
    let dispatch = drive_ready_while_outer_reader_is_held(&runtime, DISPATCH_TIMEOUT)
        .expect("the singleton Ready owner compiles while the reader is held");
    assert!(matches!(
        dispatch,
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Applied)
    ));
    runtime.with_authority_for_foundation(|authority| {
        assert!(matches!(authority.entry(&hash), Some(OwnedTx::Accepted(_))));
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_singleton_rbf_commits_while_an_outer_reader_is_held() {
    const DISPATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let snapshot = genesis_snapshot();
    let mut config = runtime_config();
    config.min_rbf_rate = FeeRate::from_u64(1_000);
    let runtime = AuthorityRuntime::new(
        &config,
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the replacement-enabled runtime fixture is valid");
    let (victim, candidate) = runtime.with_authority_for_foundation(|authority| {
        add_leaf_rbf_pair(authority, 0, 188, Vec::new(), 30_000)
    });
    let dispatch = drive_ready_while_outer_reader_is_held(&runtime, DISPATCH_TIMEOUT)
        .expect("the singleton RBF Ready result cannot require the outer writer");
    assert!(matches!(
        dispatch,
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Applied)
    ));
    runtime.with_authority_for_foundation(|authority| {
        assert!(matches!(
            authority.entry(&candidate),
            Some(OwnedTx::Accepted(_))
        ));
        assert!(matches!(
            authority.entry(&victim),
            Some(OwnedTx::ReplacementHistory(_)) | None
        ));
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_under_fee_rbf_rejection_commits_while_an_outer_reader_is_held() {
    const DISPATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let snapshot = genesis_snapshot();
    let mut config = runtime_config();
    config.min_rbf_rate = FeeRate::from_u64(1_000);
    let runtime = AuthorityRuntime::new(
        &config,
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the replacement-enabled runtime fixture is valid");
    let (victim, rejected) = runtime.with_authority_for_foundation(|authority| {
        add_leaf_rbf_pair(authority, 0, 189, Vec::new(), 100)
    });

    let dispatch = drive_ready_while_outer_reader_is_held(&runtime, DISPATCH_TIMEOUT)
        .expect("the canonical membership rejection cannot require the outer writer");
    assert!(matches!(
        dispatch,
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Applied)
    ));
    runtime.with_authority_for_foundation(|authority| {
        assert!(authority.entry(&rejected).is_none());
        assert!(matches!(
            authority.entry(&victim),
            Some(OwnedTx::Accepted(_))
        ));
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_ready_clock_contention_rolls_back_staged_effect_and_wakes_capacity() {
    const CONTENTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let runtime = runtime();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded clock-contention fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_090 + offset as u32, 1_090 + offset).await);
    }
    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);
    let held_hash = hashes
        .iter()
        .find(|hash| !selected.contains(hash))
        .expect("the fixture leaves one Ready owner outside the selected wave")
        .clone();
    let held_compiled = runtime.with_authority_read_for_foundation(|authority| {
        let batch = independent_batch(authority, std::slice::from_ref(&held_hash));
        authority
            .compile_shared_independent_settlement(&batch)
            .expect("the earlier Ready prefix compiles")
            .into_option_for_foundation()
            .expect("the earlier Ready prefix is independently commit-capable")
    });

    let interposer = admission(1_099, 1_099);
    runtime
        .admit(interposer)
        .expect("the disjoint clock interposer is admitted");
    let job = continued(
        runtime
            .try_checkout_for_foundation(WorkPermit::ResolveOnly)
            .expect("the disjoint resolve checkout remains healthy"),
    )
    .expect("the disjoint resolve job is ready");
    let completion = ComputeExchangeCompletion::from_finished(
        ComputeWorkerSlot::ordered_resolve(),
        completion(runtime.execute_compute(job)).finish_execution(),
    );

    let capacity_notified = runtime.effect_capacity_signal().notified();
    tokio::pin!(capacity_notified);
    let _ = capacity_notified.as_mut().enable();
    let (probe, clock_ready, release_clock) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_ready_clock_commit_probe(Some(probe));
    });
    let clock_before = runtime.with_authority_read_for_foundation(|authority| authority.clocks());
    let (dispatch_tx, dispatch_rx) = std::sync::mpsc::channel();
    let driver_runtime = runtime.clone();
    let driver = std::thread::spawn(move || {
        let _ = dispatch_tx.send(driver_runtime.try_drive_ready());
    });
    clock_ready
        .recv_timeout(CONTENTION_TIMEOUT)
        .expect("Ready pauses after staging but before the exact clock commit");
    let interposed = match runtime.exchange_compute(vec![completion], Vec::new()) {
        Ok(interposed) => interposed,
        Err(error) => {
            drop(error);
            panic!("the disjoint effectless compute transition advances its real clock cut")
        }
    };
    drop(interposed);
    let clock_after = runtime.with_authority_read_for_foundation(|authority| authority.clocks());
    assert!(clock_after.next_sequence > clock_before.next_sequence);
    release_clock
        .send(())
        .expect("release the stale Ready clock commit");
    let dispatch = dispatch_rx
        .recv_timeout(CONTENTION_TIMEOUT)
        .expect("the stale Ready compiler returns a typed terminal");
    driver
        .join()
        .expect("the Ready compiler thread does not panic");
    assert!(matches!(dispatch, Err(AuthorityDriverError::Stale)));
    tokio::time::timeout(CONTENTION_TIMEOUT, capacity_notified.as_mut())
        .await
        .expect("the staged suffix rollback wakes a pre-enabled capacity waiter");

    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_ready_clock_commit_probe(None);
    });
    runtime
        .cancel_unassigned_ready_jobs(vec![held_compiled])
        .expect("the earlier staged prefix cancels exactly");
    drop(excluded_reservation);
    runtime.with_authority_for_foundation(|authority| {
        assert!(selected.iter().all(|hash| {
            matches!(
                authority.entry(hash),
                Some(OwnedTx::PreAccepted(entry))
                    if matches!(entry.phase, PreAcceptedPhase::Ready(_))
            )
        }));
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
        assert!(authority.primary_projection_consistent());
    });
    let retry = runtime
        .try_drive_ready()
        .expect("the generation remains usable after typed clock contention");
    if let AuthorityReadyDispatch::Wave(wave) = retry {
        let (assignments, _wave_ends) = wave.into_parts();
        for (index, assignment) in assignments.into_iter().enumerate() {
            assert!(matches!(
                runtime.commit_ready_assignment(
                    AuthorityReadyCommitLane::from_index(index),
                    assignment,
                ),
                AuthorityReadyCommitTerminal::Applied
            ));
        }
    }
    runtime.with_authority_for_foundation(|authority| {
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_ready_wave_workers_overlap_two_real_complete_shard_cuts() {
    const CUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded overlap fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_100 + offset as u32, 1_100 + offset).await);
    }

    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_probe(Some(probe));
    });

    let attempts = Arc::new(AtomicUsize::new(0));
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let workers =
        AuthorityTestWorkerOwner::spawn_observed_ready(runtime.clone(), &handle, attempts)
            .expect("the bounded Ready topology starts atomically");
    entered
        .recv_timeout(CUT_TIMEOUT)
        .expect("the first Ready worker enters its complete cut");
    entered
        .recv_timeout(CUT_TIMEOUT)
        .expect("the disjoint second worker enters before the first releases");
    let (clear_finished, clear_observed) = std::sync::mpsc::channel();
    let clear_runtime = runtime.clone();
    let clear_task = tokio::spawn(async move {
        let result = clear_runtime.clear_pipeline().await;
        let _ = clear_finished.send(());
        result
    });
    assert!(
        clear_observed
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err(),
        "a lifecycle writer must wait outside the store while Ready jobs hold live cuts"
    );
    release.send(()).expect("release the first Ready cut");
    release.send(()).expect("release the second Ready cut");

    tokio::time::timeout(CUT_TIMEOUT, async {
        loop {
            let accepted = runtime.with_authority_for_foundation(|authority| {
                selected
                    .iter()
                    .all(|hash| matches!(authority.entry(hash), Some(OwnedTx::Accepted(_))))
            });
            if accepted {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both Ready workers commit their exact disjoint owners");
    clear_observed
        .recv_timeout(CUT_TIMEOUT)
        .expect("the lifecycle writer proceeds after per-job effect finalization");
    clear_task
        .await
        .expect("the lifecycle writer task does not panic")
        .expect("the post-wave pipeline clear remains valid");
    workers
        .shutdown()
        .await
        .expect("the bounded Ready topology drains and joins");
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_probe(None);
        assert!(authority.primary_projection_consistent());
    });
    drop(excluded_reservation);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_ready_wave_overlaps_three_real_cuts_and_conserves_every_reservation() {
    const CUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(24)
        .expect("the bounded three-cut fixture allocates");
    for offset in 0..24usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_120 + offset as u32, 1_120 + offset).await);
    }
    let selected = reserve_excluding_one_compatible_ready_triple(&runtime, &hashes);
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_probe(Some(probe));
    });

    let attempts = Arc::new(AtomicUsize::new(0));
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let workers =
        AuthorityTestWorkerOwner::spawn_observed_ready(runtime.clone(), &handle, attempts)
            .expect("the bounded Ready topology starts atomically");
    for _ in 0..selected.len() {
        entered
            .recv_timeout(CUT_TIMEOUT)
            .expect("three compatible jobs hold their complete cuts concurrently");
    }
    for _ in 0..selected.len() {
        release.send(()).expect("release one Ready cut");
    }
    tokio::time::timeout(CUT_TIMEOUT, async {
        loop {
            if runtime.with_authority_for_foundation(|authority| {
                selected
                    .iter()
                    .all(|hash| matches!(authority.entry(hash), Some(OwnedTx::Accepted(_))))
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all three independently owned Ready jobs commit");
    workers
        .shutdown()
        .await
        .expect("the three-cut Ready topology drains and joins");
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_probe(None);
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
        assert!(authority.primary_projection_consistent());
    });
    assert_eq!(runtime.effect_observation_for_foundation().queued.len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_ready_wave_driver_abort_after_dispatch_cannot_suppress_commits_or_wakes() {
    const CUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded driver-abort fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_145 + offset as u32, 1_145 + offset).await);
    }
    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_probe(Some(probe));
    });
    let publisher_wake = runtime.effect_publisher_signal().notified();
    tokio::pin!(publisher_wake);
    let _ = publisher_wake.as_mut().enable();

    let attempts = Arc::new(AtomicUsize::new(0));
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let workers =
        AuthorityTestWorkerOwner::spawn_observed_ready(runtime.clone(), &handle, attempts)
            .expect("the bounded Ready topology starts atomically");
    entered
        .recv_timeout(CUT_TIMEOUT)
        .expect("the first Ready job is owned by its permanent lane");
    entered
        .recv_timeout(CUT_TIMEOUT)
        .expect("the second Ready job is owned before the driver is aborted");
    assert!(workers.abort_role_for_foundation(AuthorityWorkerRole::Ready));
    drop(excluded_reservation);
    release.send(()).expect("release the first Ready cut");
    release.send(()).expect("release the second Ready cut");

    tokio::time::timeout(CUT_TIMEOUT, async {
        loop {
            if runtime.with_authority_for_foundation(|authority| {
                selected
                    .iter()
                    .all(|hash| matches!(authority.entry(hash), Some(OwnedTx::Accepted(_))))
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("driver result loss cannot suppress move-owned Ready commits");
    tokio::time::timeout(CUT_TIMEOUT, publisher_wake.as_mut())
        .await
        .expect("each worker publishes its own committed effect before its discarded reply");
    assert!(
        workers.shutdown().await.is_err(),
        "the aborted driver still forbids clean generation persistence"
    );
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_probe(None);
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
        assert!(authority.primary_projection_consistent());
    });
    assert_eq!(runtime.effect_observation_for_foundation().queued.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_ready_owner_cuts_commit_while_the_fair_frontier_mutex_is_held_elsewhere() {
    const COMMIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded scheduler-cut fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_160 + offset as u32, 1_160 + offset).await);
    }
    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);
    let dispatch = runtime
        .try_drive_ready()
        .expect("the compatible Ready pair compiles into worker-owned slots");
    let AuthorityReadyDispatch::Wave(wave) = dispatch else {
        panic!("the compatible Ready pair must use shared slots");
    };
    let (assignments, wave_ends) = wave.into_parts();
    assert_eq!(assignments.len(), 2);
    assert_eq!(wave_ends, vec![2]);
    let (owner_commit_probe, owner_committed, release_owner_cuts) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_owner_commit_probe(Some(owner_commit_probe));
    });

    let frontier = runtime
        .with_authority_for_foundation(|authority| authority.scheduler_frontier_for_foundation());
    let frontier_guard = frontier.lock();
    let (terminal_tx, terminal_rx) = std::sync::mpsc::channel();
    let mut handles = Vec::new();
    handles
        .try_reserve_exact(assignments.len())
        .expect("the fixed worker handles allocate");
    for (index, assignment) in assignments.into_iter().enumerate() {
        let runtime = runtime.clone();
        let terminal_tx = terminal_tx.clone();
        handles.push(std::thread::spawn(move || {
            let terminal = runtime
                .commit_ready_assignment(AuthorityReadyCommitLane::from_index(index), assignment);
            let _ = terminal_tx.send(terminal);
        }));
    }
    drop(terminal_tx);
    for _ in 0..handles.len() {
        owner_committed
            .recv_timeout(COMMIT_TIMEOUT)
            .expect("each Ready worker commits its owner cut while the scheduler mutex is held");
    }
    for _ in 0..handles.len() {
        release_owner_cuts
            .send(())
            .expect("release one owner-committed Ready worker");
    }
    drop(frontier_guard);
    let mut terminals = Vec::new();
    terminals
        .try_reserve_exact(handles.len())
        .expect("the fixed terminal carrier allocates");
    for _ in 0..handles.len() {
        terminals.push(
            terminal_rx
                .recv_timeout(COMMIT_TIMEOUT)
                .expect("post-owner wake observation resumes after the mutex is released"),
        );
    }
    for handle in handles {
        handle.join().expect("the Ready slot worker does not panic");
    }
    assert!(
        terminals
            .iter()
            .all(|terminal| matches!(terminal, AuthorityReadyCommitTerminal::Applied))
    );
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_owner_commit_probe(None);
        assert!(
            selected
                .iter()
                .all(|hash| matches!(authority.entry(hash), Some(OwnedTx::Accepted(_))))
        );
        assert!(authority.primary_projection_consistent());
    });
    drop(excluded_reservation);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_administrative_removal_retires_dispatched_ready_slots_without_resurrection() {
    let runtime = runtime();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded retirement fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_170 + offset as u32, 1_170 + offset).await);
    }
    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);
    let dispatch = runtime
        .try_drive_ready()
        .expect("the compatible Ready pair compiles into worker-owned slots");
    let AuthorityReadyDispatch::Wave(wave) = dispatch else {
        panic!("the compatible Ready pair must use shared slots");
    };
    let (assignments, _) = wave.into_parts();
    assert_eq!(assignments.len(), selected.len());

    for hash in &selected {
        assert!(
            runtime
                .remove_local_transaction(&hash.0)
                .expect("administrative removal remains coherent")
        );
    }
    let blocked = runtime.effect_observation_for_foundation();
    assert!(
        blocked.queued.is_empty(),
        "administrative effects cannot overtake the earlier Ready stages"
    );
    assert!(blocked.blocking_staged_head.is_some());
    runtime.with_authority_for_foundation(|authority| {
        assert_eq!(
            authority.effect_publication_observation_for_foundation(),
            crate::authority::effect::test_support::EffectPublicationObservationSnapshot::Idle,
            "a committed administrative suffix remains invisible behind pending Ready stages"
        );
        let trace = authority.effect_trace_for_reference();
        assert_eq!(trace.len(), selected.len());
        assert!(trace.iter().all(|batch| {
            matches!(
                batch.effects.as_slice(),
                [CommittedEffect::RemoteIngressReleased(release)]
                    if selected.iter().any(|hash| release.tx_hash() == hash)
            )
        }));
        assert!(trace.iter().all(|batch| {
            batch
                .effects
                .iter()
                .all(|effect| !matches!(effect, CommittedEffect::Accepted(_)))
        }));
    });
    for (index, assignment) in assignments.into_iter().enumerate() {
        assert!(matches!(
            runtime
                .commit_ready_assignment(AuthorityReadyCommitLane::from_index(index), assignment,),
            AuthorityReadyCommitTerminal::Stale
        ));
    }
    let exposed = runtime.effect_observation_for_foundation();
    assert_eq!(exposed.queued.len(), selected.len());
    assert!(exposed.blocking_staged_head.is_none());
    let mut released = Vec::new();
    released
        .try_reserve_exact(selected.len())
        .expect("the fixed release observation allocates");
    for _ in 0..selected.len() {
        let receipt = runtime
            .wait_effect_publication_for_foundation()
            .await
            .expect("each administrative removal publishes one exact relay release");
        let release = match receipt.effects() {
            [CommittedEffect::RemoteIngressReleased(release)] => release.tx_hash().clone(),
            effects => panic!("stale Ready workers cannot publish {effects:?}"),
        };
        released.push(release);
        runtime
            .settle_effect_for_foundation(receipt.complete_for_foundation().published())
            .expect("the exact administrative release settles");
    }
    released.sort_unstable();
    let mut expected = selected.to_vec();
    expected.sort_unstable();
    assert_eq!(released, expected);
    let settled = runtime.effect_observation_for_foundation();
    assert!(settled.queued.is_empty());
    assert!(settled.blocking_staged_head.is_none());
    assert_eq!(
        (settled.total_usage.batches, settled.total_usage.bytes),
        (0, 0)
    );
    runtime.with_authority_for_foundation(|authority| {
        assert!(selected.iter().all(|hash| authority.entry(hash).is_none()));
        assert!(authority.primary_projection_consistent());
    });
    drop(excluded_reservation);
    runtime.with_authority_for_foundation(|authority| {
        let reap = authority
            .reserve_ready_candidates()
            .expect("the post-terminal capture reaps every terminal slot claim");
        drop(reap);
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
        let (_, physical_reserved, physical_claims) =
            authority.ready_physical_counts_for_foundation();
        assert_eq!((physical_reserved, physical_claims), (0, 0));
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_ready_wave_closed_lane_cancels_failed_and_unsent_jobs_exactly() {
    const TERMINAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded closed-lane fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_170 + offset as u32, 1_170 + offset).await);
    }
    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);
    let capacity_wake = runtime.effect_capacity_signal().notified();
    tokio::pin!(capacity_wake);
    let _ = capacity_wake.as_mut().enable();

    let attempts = Arc::new(AtomicUsize::new(0));
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let workers = AuthorityTestWorkerOwner::spawn_observed_ready_with_closed_lane(
        runtime.clone(),
        &handle,
        attempts,
        AuthorityReadyCommitLane::First,
    )
    .expect("the intentionally incomplete Ready topology starts");
    tokio::time::timeout(TERMINAL_TIMEOUT, capacity_wake.as_mut())
        .await
        .expect("transport failure returns every staged effect charge");
    drop(excluded_reservation);
    assert!(
        workers.shutdown().await.is_err(),
        "a missing permanent lane is a generation integrity fault"
    );
    runtime.with_authority_for_foundation(|authority| {
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
        assert!(
            selected
                .iter()
                .all(|hash| !matches!(authority.entry(hash), Some(OwnedTx::Accepted(_))))
        );
        assert!(authority.primary_projection_consistent());
    });
    assert!(
        runtime
            .effect_observation_for_foundation()
            .queued
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_ready_wave_publishes_committed_rows_before_capacity_fault_supervision() {
    const PAIR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded capacity-fault fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_160 + offset as u32, 1_160 + offset).await);
    }
    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_probe(Some(probe));
    });
    let publisher_wake = runtime.effect_publisher_signal().notified();
    tokio::pin!(publisher_wake);
    let _ = publisher_wake.as_mut().enable();

    let attempts = Arc::new(AtomicUsize::new(0));
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let workers =
        AuthorityTestWorkerOwner::spawn_observed_ready(runtime.clone(), &handle, attempts)
            .expect("the bounded Ready topology starts atomically");
    entered
        .recv_timeout(PAIR_TIMEOUT)
        .expect("one lane begins its exact capacity commit");
    entered
        .recv_timeout(PAIR_TIMEOUT)
        .expect("another lane begins before the injected absorbing fault");
    runtime.with_authority_read_for_foundation(|authority| {
        authority.resources().fault_capacity_for_foundation();
    });
    release.send(()).expect("release the first owner cut");
    release.send(()).expect("release the sibling owner cut");

    tokio::time::timeout(PAIR_TIMEOUT, publisher_wake.as_mut())
        .await
        .expect("committed rows activate their effects before fault supervision");
    assert!(
        workers.shutdown().await.is_err(),
        "the Ready supervisor must observe the post-commit resource fault"
    );
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_probe(None);
        assert!(
            selected
                .iter()
                .all(|hash| matches!(authority.entry(hash), Some(OwnedTx::Accepted(_))))
        );
        assert_eq!(
            authority.primary_projection_inconsistencies(),
            vec!["resources"],
            "the absorbing capacity fault cannot cancel committed owners or corrupt another projection"
        );
    });
    assert_eq!(runtime.effect_observation_for_foundation().queued.len(), 2);
    drop(excluded_reservation);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_capacity_coupled_ready_commits_without_the_outer_writer() {
    const DISPATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let runtime = runtime_with_one_accepted_owner();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded capacity fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_180 + offset as u32, 1_180 + offset).await);
    }
    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);

    assert!(matches!(
        drive_ready_while_outer_reader_is_held(&runtime, DISPATCH_TIMEOUT)
            .expect("capacity policy commits its exact frontier without the outer writer"),
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Applied)
    ));
    runtime.with_authority_for_foundation(|authority| {
        let accepted = selected
            .iter()
            .filter(|hash| matches!(authority.entry(hash), Some(OwnedTx::Accepted(_))))
            .count();
        assert_eq!(
            accepted, 1,
            "the capacity limit commits only the strongest owner"
        );
        assert!(authority.primary_projection_consistent());
    });
    drop(excluded_reservation);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_ready_wave_effect_batch_coupling_falls_back_to_one_canonical_commit() {
    let runtime = runtime_with_one_effect_batch();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded effect fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_270 + offset as u32, 1_270 + offset).await);
    }
    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);

    assert!(matches!(
        runtime
            .try_drive_ready()
            .expect("per-job effect pressure returns to the canonical aggregate"),
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Applied)
    ));
    runtime.with_authority_for_foundation(|authority| {
        assert!(
            selected
                .iter()
                .all(|hash| matches!(authority.entry(hash), Some(OwnedTx::Accepted(_))))
        );
        assert_eq!(
            authority.ready_reserved_len_for_foundation(),
            hashes.len() - selected.len()
        );
        assert!(authority.primary_projection_consistent());
    });
    assert_eq!(
        runtime.effect_observation_for_foundation().queued.len(),
        1,
        "the canonical aggregate consumes one indivisible effect batch"
    );
    drop(excluded_reservation);
    runtime.with_authority_for_foundation(|authority| {
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_ready_same_peer_aggregate_uses_one_canonical_apply_not_a_stale_later_wave() {
    let runtime = runtime();
    let peer = 1_295usize;
    let first = advance_remote_to_ready(&runtime, 1_295, peer).await;
    let second = advance_remote_to_ready(&runtime, 1_296, peer).await;

    assert!(matches!(
        runtime
            .try_drive_ready()
            .expect("the shared peer aggregate uses the canonical cohort Apply"),
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Applied)
    ));
    runtime.with_authority_for_foundation(|authority| {
        assert!(matches!(
            authority.entry(&first),
            Some(OwnedTx::Accepted(_))
        ));
        assert!(matches!(
            authority.entry(&second),
            Some(OwnedTx::Accepted(_))
        ));
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
        assert!(authority.primary_projection_consistent());
    });
    assert_eq!(
        runtime.effect_observation_for_foundation().queued.len(),
        1,
        "the two same-peer effects retain one canonical aggregate publication"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_ready_wave_preserves_trusted_progress_when_remote_effect_region_is_full() {
    let runtime = runtime_with_one_remote_effect_batch_and_trusted_headroom();
    let mut remote_hashes = Vec::new();
    let mut trusted_hashes = Vec::new();
    remote_hashes
        .try_reserve_exact(8)
        .expect("the bounded remote fixture allocates");
    trusted_hashes
        .try_reserve_exact(8)
        .expect("the bounded trusted fixture allocates");
    for offset in 0..8usize {
        remote_hashes
            .push(advance_remote_to_ready(&runtime, 1_300 + offset as u32, 1_300 + offset).await);
        let trusted = ValidatedAdmission::proposal(
            TransactionBuilder::default()
                .version(1_400 + offset as u32)
                .build(),
        )
        .expect("the trusted cross-class fixture has valid ingress evidence");
        trusted_hashes.push(advance_admission_to_ready(&runtime, trusted).await);
    }
    let selected = reserve_excluding_one_compatible_cross_class_ready_wave(
        &runtime,
        &remote_hashes,
        &trusted_hashes,
    );
    queue_remote_rejection(&runtime, 1_299);
    let dispatch = runtime
        .try_drive_ready()
        .expect("remote saturation cannot hide trusted Ready progress");
    let AuthorityReadyDispatch::Wave(wave) = dispatch else {
        panic!("the trusted prefix remains eligible for shared Apply");
    };
    let (assignments, wave_ends) = wave.into_parts();
    assert_eq!(assignments.len(), 1);
    assert_eq!(wave_ends, vec![1]);
    for (index, assignment) in assignments.into_iter().enumerate() {
        assert!(matches!(
            runtime
                .commit_ready_assignment(AuthorityReadyCommitLane::from_index(index), assignment,),
            AuthorityReadyCommitTerminal::Applied
        ));
    }
    runtime.with_authority_for_foundation(|authority| {
        assert!(matches!(
            authority.entry(&selected[0]),
            Some(OwnedTx::Accepted(_))
        ));
        assert!(matches!(
            authority.entry(&selected[1]),
            Some(OwnedTx::PreAccepted(entry)) if matches!(entry.phase, PreAcceptedPhase::Ready(_))
        ));
        assert!(authority.primary_projection_consistent());
    });
    let effects = runtime.effect_observation_for_foundation();
    assert_eq!(effects.queued.len(), 2);
    assert_eq!(effects.remote_usage.batches, 1);
    assert_eq!(effects.ordinary_usage.batches, 2);
    assert_eq!(effects.total_usage.batches, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_ready_wave_stale_prefix_releases_capacity_and_wakes_committed_suffix() {
    const PAIR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded stale-prefix fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_200 + offset as u32, 1_200 + offset).await);
    }
    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);
    let [stale_hash, committed_hash] = &selected;
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.set_ready_commit_probe_for_foundation(AuthorityReadyCommitLane::First, Some(probe));

    let attempts = Arc::new(AtomicUsize::new(0));
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let workers =
        AuthorityTestWorkerOwner::spawn_observed_ready(runtime.clone(), &handle, attempts)
            .expect("the bounded Ready topology starts atomically");
    entered
        .recv_timeout(PAIR_TIMEOUT)
        .expect("the stronger lane pauses before binding its exact cut");

    tokio::time::timeout(PAIR_TIMEOUT, async {
        loop {
            if runtime.with_authority_for_foundation(|authority| {
                matches!(authority.entry(committed_hash), Some(OwnedTx::Accepted(_)))
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the compatible suffix commits while the prefix lane is paused");

    let stronger =
        ValidatedAdmission::proposal(TransactionBuilder::default().version(1_299u32).build())
            .expect("the trusted priority canary has valid ingress evidence");
    let _stronger_hash = advance_admission_to_ready(&runtime, stronger).await;
    let publisher_wake = runtime.effect_publisher_signal().notified();
    let capacity_wake = runtime.effect_capacity_signal().notified();
    tokio::pin!(publisher_wake);
    tokio::pin!(capacity_wake);
    let _ = publisher_wake.as_mut().enable();
    let _ = capacity_wake.as_mut().enable();
    workers.cancellation_for_foundation().cancel();
    release.send(()).expect("release the now-stale prefix lane");
    tokio::time::timeout(PAIR_TIMEOUT, publisher_wake.as_mut())
        .await
        .expect("cancelling the prefix makes the committed suffix publishable");
    tokio::time::timeout(PAIR_TIMEOUT, capacity_wake.as_mut())
        .await
        .expect("cancelling the prefix returns its exact effect capacity");

    workers
        .shutdown()
        .await
        .expect("the stale-prefix topology drains and joins");
    runtime.set_ready_commit_probe_for_foundation(AuthorityReadyCommitLane::First, None);
    runtime.with_authority_for_foundation(|authority| {
        assert!(matches!(
            authority.entry(committed_hash),
            Some(OwnedTx::Accepted(_))
        ));
        assert!(
            !matches!(authority.entry(stale_hash), Some(OwnedTx::Accepted(_))),
            "the stronger-prefix row must not commit from stale priority evidence"
        );
        assert!(authority.primary_projection_consistent());
    });
    drop(excluded_reservation);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_one_ready_attempt_commits_a_bounded_coupled_sibling_batch() {
    const DISPATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let runtime = runtime();
    let (parent, children) = runtime.with_authority_for_foundation(|authority| {
        accepted_parent_with_ready_children(authority, 120, MAX_READY_BATCH)
    });
    assert_eq!(children.len(), MAX_READY_BATCH);
    assert!(runtime.with_authority_for_foundation(|authority| {
        children.iter().all(|child| {
            matches!(
                authority.entry(child),
                Some(OwnedTx::PreAccepted(entry))
                    if matches!(entry.phase, PreAcceptedPhase::Ready(_))
            )
        })
    }));

    assert!(matches!(
        drive_ready_while_outer_reader_is_held(&runtime, DISPATCH_TIMEOUT)
            .expect("the bounded coupled Ready batch commits without the outer writer"),
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Applied)
    ));

    runtime.with_authority_for_foundation(|authority| {
        assert!(matches!(
            authority.entry(&parent),
            Some(OwnedTx::Accepted(_))
        ));
        assert!(
            children
                .iter()
                .all(|child| matches!(authority.entry(child), Some(OwnedTx::Accepted(_))))
        );
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_effect_capacity_resumes_the_exact_coupled_ready_tail() {
    let runtime = runtime_with_one_effect_batch();
    let (_parent, children) = runtime.with_authority_for_foundation(|authority| {
        accepted_parent_with_ready_children(authority, 121, MAX_READY_BATCH)
    });

    let mut continuation = match runtime
        .try_drive_ready()
        .expect("effect pressure is an owned Ready outcome")
    {
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::EffectCapacity(continuation)) => {
            continuation
        }
        outcome => panic!("the resident parent effect must block Ready, got {outcome:?}"),
    };

    let mut accepted = runtime.with_authority_for_foundation(|authority| {
        children
            .iter()
            .filter(|child| matches!(authority.entry(child), Some(OwnedTx::Accepted(_))))
            .count()
    });
    assert!(accepted < children.len());

    while accepted < children.len() {
        let occupied = runtime
            .wait_effect_publication_for_foundation()
            .await
            .expect("one prior effect owns the only resident slot");
        runtime
            .settle_effect_for_foundation(occupied.complete_for_foundation().published())
            .expect("the exact prior publication releases capacity");

        let outcome = runtime
            .resume_ready(continuation)
            .expect("the exact retained Ready tail revalidates");
        let next_accepted = runtime.with_authority_for_foundation(|authority| {
            children
                .iter()
                .filter(|child| matches!(authority.entry(child), Some(OwnedTx::Accepted(_))))
                .count()
        });
        assert!(next_accepted > accepted);
        accepted = next_accepted;

        match outcome {
            AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::EffectCapacity(next)) => {
                assert!(accepted < children.len());
                continuation = next;
            }
            AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Applied) => {
                assert_eq!(accepted, children.len());
                break;
            }
            AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Idle) => {
                panic!("a non-empty retained Ready tail cannot become idle")
            }
            AuthorityReadyDispatch::Wave(_) => {
                panic!("the one-slot effect envelope cannot dispatch a Ready wave")
            }
        }
    }

    runtime.with_authority_for_foundation(|authority| {
        assert!(
            children
                .iter()
                .all(|child| matches!(authority.entry(child), Some(OwnedTx::Accepted(_))))
        );
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_ready_worker_resumes_capacity_blocked_tail_after_publication_release() {
    let runtime = runtime_with_one_effect_batch();
    let (_parent, children) = runtime.with_authority_for_foundation(|authority| {
        accepted_parent_with_ready_children(authority, 124, MAX_READY_BATCH)
    });
    let attempts = Arc::new(AtomicUsize::new(0));
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let workers = AuthorityTestWorkerOwner::spawn_observed_ready(
        runtime.clone(),
        &handle,
        Arc::clone(&attempts),
    )
    .expect("the test owns the capacity-blocked Ready worker");

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let accepted = runtime.with_authority_for_foundation(|authority| {
                assert!(authority.primary_projection_consistent());
                children
                    .iter()
                    .filter(|child| matches!(authority.entry(child), Some(OwnedTx::Accepted(_))))
                    .count()
            });
            if accepted == children.len() {
                break;
            }
            let occupied = runtime
                .wait_effect_publication_for_foundation()
                .await
                .expect("the blocked Ready worker publishes one bounded effect batch");
            runtime
                .settle_effect_for_foundation(occupied.complete_for_foundation().published())
                .expect("publication settlement releases the existing capacity signal");
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("capacity releases drive the exact retained tail to completion");

    workers
        .shutdown()
        .await
        .expect("the capacity-blocked Ready worker cancels and joins cleanly");
    assert!(
        attempts.load(Ordering::Relaxed) > 1,
        "the real worker must cross at least one effect-capacity wait"
    );
    runtime.with_authority_for_foundation(|authority| {
        assert!(
            children
                .iter()
                .all(|child| matches!(authority.entry(child), Some(OwnedTx::Accepted(_))))
        );
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_effect_capacity_continuation_refuses_a_cleared_ready_generation() {
    let runtime = runtime_with_one_effect_batch();
    let (_parent, children) = runtime.with_authority_for_foundation(|authority| {
        accepted_parent_with_ready_children(authority, 122, MAX_READY_BATCH)
    });
    let continuation = match runtime
        .try_drive_ready()
        .expect("effect pressure is an owned Ready outcome")
    {
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::EffectCapacity(continuation)) => {
            continuation
        }
        outcome => panic!("the resident parent effect must block Ready, got {outcome:?}"),
    };

    let occupied = runtime
        .wait_effect_publication_for_foundation()
        .await
        .expect("one prior effect owns the only resident slot");
    runtime
        .settle_effect_for_foundation(occupied.complete_for_foundation().published())
        .expect("the exact prior publication releases capacity");
    let accepted_before_clear = runtime.with_authority_for_foundation(|authority| {
        children
            .iter()
            .filter(|child| matches!(authority.entry(child), Some(OwnedTx::Accepted(_))))
            .count()
    });
    runtime
        .clear_pipeline()
        .await
        .expect("the generation clear owns its exact administrative cut");

    assert!(matches!(
        runtime.resume_ready(continuation),
        Err(AuthorityDriverError::Stale)
    ));
    runtime.with_authority_for_foundation(|authority| {
        let accepted_after_clear = children
            .iter()
            .filter(|child| matches!(authority.entry(child), Some(OwnedTx::Accepted(_))))
            .count();
        assert_eq!(accepted_after_clear, accepted_before_clear);
        assert!(
            children
                .iter()
                .all(|child| { !matches!(authority.entry(child), Some(OwnedTx::PreAccepted(_))) })
        );
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_dropped_effect_capacity_continuation_returns_to_the_ready_level() {
    let runtime = runtime_with_one_effect_batch();
    let (_parent, children) = runtime.with_authority_for_foundation(|authority| {
        accepted_parent_with_ready_children(authority, 123, MAX_READY_BATCH)
    });
    let continuation = match runtime
        .try_drive_ready()
        .expect("effect pressure is an owned Ready outcome")
    {
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::EffectCapacity(continuation)) => {
            continuation
        }
        outcome => panic!("the resident parent effect must block Ready, got {outcome:?}"),
    };
    let accepted_before_drop = runtime.with_authority_for_foundation(|authority| {
        children
            .iter()
            .filter(|child| matches!(authority.entry(child), Some(OwnedTx::Accepted(_))))
            .count()
    });
    let notified = runtime.ready_signal().notified();
    tokio::pin!(notified);
    let _ = notified.as_mut().enable();
    drop(continuation);
    tokio::time::timeout(std::time::Duration::from_millis(100), notified.as_mut())
        .await
        .expect("dropping live Ready ownership republishes the exact Ready level");

    let occupied = runtime
        .wait_effect_publication_for_foundation()
        .await
        .expect("one prior effect owns the only resident slot");
    runtime
        .settle_effect_for_foundation(occupied.complete_for_foundation().published())
        .expect("the exact prior publication releases capacity");
    let fresh = runtime
        .try_drive_ready()
        .expect("the level-triggered Ready frontier recaptures dropped receipts");
    assert!(matches!(
        fresh,
        AuthorityReadyDispatch::Outcome(
            AuthorityReadyOutcome::Applied | AuthorityReadyOutcome::EffectCapacity(_)
        )
    ));
    let accepted_after_recapture = runtime.with_authority_for_foundation(|authority| {
        assert!(authority.primary_projection_consistent());
        children
            .iter()
            .filter(|child| matches!(authority.entry(child), Some(OwnedTx::Accepted(_))))
            .count()
    });
    assert!(accepted_after_recapture > accepted_before_drop);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_ready_recheck_keeps_the_unchanged_head_when_a_weaker_tail_disappears() {
    let runtime = runtime();
    let head = advance_remote_to_ready(&runtime, 915, 105).await;
    let tail = advance_remote_to_ready(&runtime, 916, 106).await;
    let work = {
        let store = runtime.store.read();
        assert_eq!(
            store
                .authority
                .ready_candidates()
                .expect("the scheduler projection is valid")
                .into_iter()
                .map(|(key, _)| key)
                .collect::<Vec<_>>(),
            vec![head.clone(), tail]
        );
        store
            .capture_ready_work_batch()
            .expect("the first Ready cut is valid")
            .expect("the Ready cut is non-empty")
    };
    let prepared = work
        .prepare(FeeRate::zero())
        .expect("Ready scratch allocation succeeds");

    {
        let mut store = runtime.store.write();
        drop(
            store
                .authority
                .plan_peer_revocation_for_foundation(PeerIndex::from(106))
                .expect("the weaker peer cohort can retire")
                .apply(),
        );
    }

    let rechecked = {
        let store = runtime.store.read();
        store.complete_ready_batch(prepared)
    }
    .expect("a changed weaker tail cannot stale the unchanged strongest prefix");
    let rechecked = rechecked
        .finish()
        .expect("the unchanged strongest prefix remains executable");
    assert_eq!(rechecked.tail.len(), 0);
    let ReadyDisposition::Candidates {
        batch,
        reservation: _,
    } = rechecked
        .validate()
        .expect("the unchanged strongest candidate remains valid")
    else {
        panic!("the unchanged strongest candidate remains the Ready disposition")
    };
    assert_eq!(batch.len(), 1);
    assert!(runtime.store.read().authority.entry(&head).is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_ready_recheck_never_skips_a_new_stronger_head() {
    let runtime = runtime();
    let weaker = advance_remote_to_ready(&runtime, 917, 107).await;
    let work = {
        let store = runtime.store.read();
        store
            .capture_ready_work_batch()
            .expect("the first Ready cut is valid")
            .expect("the Ready cut is non-empty")
    };
    let prepared = work
        .prepare(FeeRate::zero())
        .expect("Ready scratch allocation succeeds");

    let stronger_admission =
        ValidatedAdmission::proposal(TransactionBuilder::default().version(918u32).build())
            .expect("the trusted stronger fixture has valid ingress evidence");
    let stronger = advance_admission_to_ready(&runtime, stronger_admission).await;
    {
        let store = runtime.store.read();
        assert_eq!(
            store
                .authority
                .ready_candidates()
                .expect("the scheduler projection is valid")
                .into_iter()
                .map(|(key, _)| key)
                .collect::<Vec<_>>(),
            vec![stronger, weaker]
        );
    }

    let rechecked = {
        let store = runtime.store.read();
        store.complete_ready_batch(prepared)
    }
    .expect("a changed head is an ordinary Ready cut outcome");
    assert!(
        rechecked.finish().is_none(),
        "the earlier weaker cut cannot pass a new stronger head"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_ready_apply_rejects_a_new_stronger_head_after_recheck() {
    let runtime = runtime();
    let weaker = advance_remote_to_ready(&runtime, 919, 109).await;
    let work = {
        let store = runtime.store.read();
        store
            .capture_ready_work_batch()
            .expect("the first Ready cut is valid")
            .expect("the Ready cut is non-empty")
    };
    let prepared = work
        .prepare(FeeRate::zero())
        .expect("Ready scratch allocation succeeds");
    let rechecked = {
        let store = runtime.store.read();
        store.complete_ready_batch(prepared)
    }
    .expect("the unchanged reservation rechecks")
    .finish()
    .expect("the original Ready head remains current");
    let ReadyDisposition::Candidates { batch, reservation } = rechecked
        .validate()
        .expect("the original candidate validates")
    else {
        panic!("the original candidate remains an ordinary settlement")
    };

    let stronger_admission =
        ValidatedAdmission::proposal(TransactionBuilder::default().version(920u32).build())
            .expect("the trusted stronger fixture has valid ingress evidence");
    let stronger = advance_admission_to_ready(&runtime, stronger_admission).await;
    let notified = runtime.ready_signal().notified();
    tokio::pin!(notified);
    let _ = notified.as_mut().enable();
    assert!(matches!(
        runtime
            .apply_ready_input(ReservedReadyPlanInput {
                input: ReadyPlanInput::Initial(batch),
                reservation,
            })
            .expect("the final priority race is ordinary OCC staleness"),
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Applied)
    ));
    tokio::time::timeout(std::time::Duration::from_millis(100), notified.as_mut())
        .await
        .expect("returning the stale reservation republishes the exact Ready level");
    runtime.with_authority_for_foundation(|authority| {
        assert!(matches!(
            authority.entry(&weaker),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Ready(_))
        ));
        assert!(matches!(
            authority.entry(&stronger),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Ready(_))
        ));
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_ready_stale_release_wake_publishes_after_releasing_the_outer_reader() {
    const DEADLOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
    const WRITER_PARK_GRACE: std::time::Duration = std::time::Duration::from_millis(300);
    let runtime = runtime();
    let weaker = advance_remote_to_ready(&runtime, 921, 111).await;
    let work = {
        let store = runtime.store.read();
        store
            .capture_ready_work_batch()
            .expect("the first Ready cut is valid")
            .expect("the Ready cut is non-empty")
    };
    let prepared = work
        .prepare(FeeRate::zero())
        .expect("Ready scratch allocation succeeds");
    let rechecked = {
        let store = runtime.store.read();
        store.complete_ready_batch(prepared)
    }
    .expect("the unchanged reservation rechecks")
    .finish()
    .expect("the original Ready head remains current");
    let ReadyDisposition::Candidates { batch, reservation } = rechecked
        .validate()
        .expect("the original candidate validates")
    else {
        panic!("the original candidate remains an ordinary settlement")
    };
    let stronger_admission =
        ValidatedAdmission::proposal(TransactionBuilder::default().version(922u32).build())
            .expect("the trusted stronger fixture has valid ingress evidence");
    let stronger = advance_admission_to_ready(&runtime, stronger_admission).await;

    let (probe, clock_ready, release_clock) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_ready_clock_commit_probe(Some(probe));
    });
    let (dispatch_tx, dispatch_rx) = std::sync::mpsc::channel();
    let driver_runtime = runtime.clone();
    let driver = std::thread::spawn(move || {
        let _ = dispatch_tx.send(driver_runtime.apply_ready_input(ReservedReadyPlanInput {
            input: ReadyPlanInput::Initial(batch),
            reservation,
        }));
    });
    clock_ready
        .recv_timeout(DEADLOCK_TIMEOUT)
        .expect("the singleton Ready compile pauses under its outer reader");

    let (writer_acquired_tx, writer_acquired_rx) = std::sync::mpsc::channel();
    let writer_runtime = runtime.clone();
    let writer = std::thread::spawn(move || {
        let guard = writer_runtime.store.write();
        let _ = writer_acquired_tx.send(());
        drop(guard);
    });
    std::thread::sleep(WRITER_PARK_GRACE);
    assert!(
        writer_acquired_rx.try_recv().is_err(),
        "the queued writer parks behind the driver's outer read guard"
    );
    release_clock
        .send(())
        .expect("release the Ready clock commit probe");

    let Ok(dispatch) = dispatch_rx.recv_timeout(DEADLOCK_TIMEOUT) else {
        // Do not block the failing test process on the deliberately wedged
        // lock graph. The leaked fixture is test-only evidence of the liveness
        // failure, never a production recovery mechanism.
        std::mem::forget(runtime);
        std::mem::forget(writer);
        std::mem::forget(driver);
        panic!(
            "the stale Ready driver self-deadlocked by re-reading the fair store lock while its outer reader blocked a queued writer"
        );
    };
    assert!(matches!(
        dispatch.expect("the final priority race is ordinary OCC staleness"),
        AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Applied)
    ));
    driver
        .join()
        .expect("the Ready driver thread does not panic");
    writer_acquired_rx
        .recv_timeout(DEADLOCK_TIMEOUT)
        .expect("the queued writer proceeds after the outer reader is released");
    writer.join().expect("the writer thread does not panic");
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_ready_clock_commit_probe(None);
        assert!(matches!(
            authority.entry(&weaker),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Ready(_))
        ));
        assert!(matches!(
            authority.entry(&stronger),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Ready(_))
        ));
        assert!(authority.primary_projection_consistent());
    });
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
        ControlFlow::Continue(Ok(job)) => job,
        ControlFlow::Continue(Err(execution)) => {
            drop(execution);
            panic!("the retained fixture has queued resolve work")
        }
        ControlFlow::Break(_) => panic!("the fixture has sufficient effect capacity"),
    };

    let direct_execution = runtime
        .try_compute_execution_for_foundation()
        .expect("direct work shares the same partition");
    let direct_tx = TransactionBuilder::default().version(1u32).build();
    let direct_input = BoundedTransaction::try_new(direct_tx)
        .expect("direct fixture transaction is bounded")
        .into_direct();
    let AuthorityDirectResolutionOutcome::Rejected(direct) = runtime
        .resolve_test_accept_transaction(&direct_input, direct_execution)
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
    let workers = AuthorityTestWorkerOwner::spawn_set(
        runtime.clone(),
        &handle,
        cache,
        cache_tx,
        ChunkCommand::Suspend,
    )
    .expect("the validated worker topology reserves its handle vector");
    assert_eq!(workers.verifier_count(), 4);

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

    workers
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
        .expect("the cache receiver responds")
        .expect("the cache receiver remains open");
    let expected_witness: [u8; 32] = TransactionBuilder::default()
        .version(906u32)
        .build()
        .witness_hash()
        .unpack();
    assert_eq!(update.into_proof().key().witness_hash(), &expected_witness);

    workers
        .shutdown()
        .await
        .expect("the structured worker generation closes cleanly");
    assert!(
        runtime
            .store
            .read()
            .authority
            .primary_projection_consistent()
    );
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
async fn runtime_compute_coordinator_drains_a_coalesced_preexisting_frontier() {
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
    let workers = AuthorityTestWorkerOwner::spawn_set(
        runtime.clone(),
        &handle,
        cache,
        cache_tx,
        ChunkCommand::Resume,
    )
    .expect("the validated topology reserves its handle vector");

    let drained = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let committed = runtime.post_commit_signal_for_foundation().notified();
            tokio::pin!(committed);
            committed.as_mut().enable();
            let accepted = {
                let store = runtime.store.read();
                keys.iter()
                    .filter(|key| matches!(store.authority.entry(key), Some(OwnedTx::Accepted(_))))
                    .count()
            };
            if accepted == keys.len() {
                break;
            }
            committed.as_mut().await;
        }
    })
    .await;
    if drained.is_err() {
        let (summary, active_work) = {
            let store = runtime.store.read();
            (
                store
                    .authority
                    .read_view()
                    .summary()
                    .expect("the stalled fixture retains a coherent read summary"),
                store.authority.resources().preaccepted().active_work,
            )
        };
        let worker_shutdown = workers.shutdown().await;
        panic!(
            "the bounded coordinator stalled: {summary:?}, active_work={active_work}, available_permits={}, worker_shutdown={worker_shutdown:?}",
            runtime.available_compute_permits_for_foundation(),
        );
    }
    let accepted = {
        let store = runtime.store.read();
        keys.iter()
            .filter(|key| matches!(store.authority.entry(key), Some(OwnedTx::Accepted(_))))
            .count()
    };
    assert_eq!(
        accepted, TRANSACTIONS,
        "every post-commit terminal must be visible in the authority"
    );

    workers
        .shutdown()
        .await
        .expect("the structured worker generation closes cleanly");
    assert!(
        runtime
            .store
            .read()
            .authority
            .primary_projection_consistent()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn runtime_single_any_verifier_settles_mixed_preexisting_frontier() {
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
        let completion = completion(runtime.execute_compute(job));
        drop(continued(runtime.settle_completion(completion)));
    }

    // Consume every coalesced hint without doing work. The sealed topology
    // must still recover from the authoritative levels through its initial
    // probe; notifications never become a second work authority.
    expect_signal(
        runtime.compute_signal(),
        "the mixed frontier retains one coalesced compute hint",
    )
    .await;
    expect_no_signal(
        runtime.compute_signal(),
        "role projections never create a second transport authority",
    )
    .await;

    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let cache = Arc::new(TokioRwLock::new(ckb_verification::cache::init_cache()));
    let (cache_tx, _cache_rx) = mpsc::channel(TRANSACTIONS);
    let workers = AuthorityTestWorkerOwner::spawn_set(
        runtime.clone(),
        &handle,
        cache,
        cache_tx,
        ChunkCommand::Resume,
    )
    .expect("the single-verifier worker set starts");

    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let committed = runtime.post_commit_signal_for_foundation().notified();
            tokio::pin!(committed);
            committed.as_mut().enable();
            let pending = {
                let store = runtime.store.read();
                keys.iter()
                    .any(|key| matches!(store.authority.entry(key), Some(OwnedTx::PreAccepted(_))))
            };
            if !pending {
                break;
            }
            committed.as_mut().await;
        }
    })
    .await
    .expect("the remaining rejected terminals eventually leave the preaccepted frontier");

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

    workers
        .shutdown()
        .await
        .expect("the structured worker generation closes cleanly");
    assert!(
        runtime
            .store
            .read()
            .authority
            .primary_projection_consistent()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_worker_retains_rejected_settlement_until_effect_capacity_returns() {
    const EFFECT_BYTES: usize = 1024 * 1024;
    let mut config = runtime_config();
    config.min_fee_rate = FeeRate::from_u64(1_000);
    config.max_tx_verify_workers = 1;
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
    let workers = AuthorityTestWorkerOwner::spawn_set(
        runtime.clone(),
        &handle,
        cache,
        cache_tx,
        ChunkCommand::Resume,
    )
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
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if runtime.available_compute_permits_for_foundation() == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("an effect-blocked completion returns its fair compute permit");

    let occupied_lease = runtime
        .wait_effect_publication_for_foundation()
        .await
        .expect("the occupied effect is queued");
    runtime
        .settle_effect_for_foundation(occupied_lease.complete_for_foundation().published())
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

    workers
        .shutdown()
        .await
        .expect("the structured worker generation closes cleanly");
}

#[tokio::test]
async fn runtime_effect_boundary_retains_and_drains_a_closed_log_in_sequence() {
    let runtime = runtime();
    queue_remote_rejection(&runtime, 909);
    queue_remote_rejection(&runtime, 910);

    let first = runtime
        .wait_effect_publication_for_foundation()
        .await
        .expect("the first effect is committed");
    let first_sequence = first.sequence();
    runtime
        .close_effects()
        .await
        .expect("zero active compute permits effect close");
    assert!(!runtime.effects_closed_and_drained());
    assert_eq!(
        runtime.admit(admission(911, 99)).err(),
        Some(PlanError::EffectClosed),
        "closing the effect authority freezes new state producers"
    );

    runtime
        .settle_effect_for_foundation(first.retain())
        .expect("Retain commits the exact tentative cursor to the resident head");
    let retained = runtime
        .wait_effect_publication_for_foundation()
        .await
        .expect("the retained head is still committed");
    assert_eq!(retained.sequence(), first_sequence);
    runtime
        .settle_effect_for_foundation(retained.complete_for_foundation().published())
        .expect("the retained head publishes exactly once");

    let second = runtime
        .wait_effect_publication_for_foundation()
        .await
        .expect("the second effect remains queued after close");
    assert!(second.sequence() > first_sequence);
    assert!(!runtime.effects_closed_and_drained());
    runtime
        .settle_effect_for_foundation(second.complete_for_foundation().circuit_disposed())
        .expect("a stable endpoint circuit may dispose its exact batch");

    assert!(
        runtime
            .wait_effect_publication_for_foundation()
            .await
            .is_none()
    );
    assert!(runtime.effects_closed_and_drained());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_direct_rejection_shared_guard_fences_effect_close_until_activation() {
    const TERMINAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    let runtime = runtime();
    let transaction = TransactionBuilder::default().version(1u32).build();
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture has one Direct execution slot");
    let direct = BoundedTransaction::try_new(transaction.clone())
        .expect("the stable rejection fixture is bounded")
        .into_direct();
    let AuthorityDirectResolutionOutcome::Rejected(rejection) = runtime
        .resolve_local_transaction(&direct, execution)
        .expect("the stable rejection is typed")
    else {
        panic!("the non-zero transaction version rejects before resolution")
    };
    let (probe, staged, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::DirectRejectionEffectStagedBeforeReadCut,
            Some(probe),
        );
    });
    let commit_runtime = runtime.clone();
    let (terminal_tx, terminal_rx) = std::sync::mpsc::sync_channel(1);
    let terminal = std::thread::spawn(move || {
        terminal_tx
            .send(commit_runtime.settle_direct_transaction_rejection(rejection))
            .expect("the Direct terminal observer remains alive");
    });
    staged
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("the Direct terminal is staged under its shared outer guard");

    let close_runtime = runtime.clone();
    let close = tokio::spawn(async move { close_runtime.close_effects().await });
    tokio::time::timeout(TERMINAL_TIMEOUT, async {
        loop {
            if runtime.lifecycle_fence.state.lock().lifecycle_writer_active {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("effect close reaches the generation writer boundary");
    assert!(
        !close.is_finished(),
        "effect close cannot pass the Direct terminal's shared generation guard"
    );
    release.send(()).expect("release Direct effect activation");
    assert!(matches!(
        terminal_rx
            .recv_timeout(TERMINAL_TIMEOUT)
            .expect("the Direct terminal returns before close"),
        Ok(AuthorityDirectRejectionExecution::Local(_))
    ));
    terminal.join().expect("the Direct terminal does not panic");
    close
        .await
        .expect("the close task remains healthy")
        .expect("effect close follows the completed Direct terminal");
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::DirectRejectionEffectStagedBeforeReadCut,
            None,
        );
    });
    assert!(
        runtime
            .pending_recent_reject(&transaction.hash())
            .expect("the closed journal projection remains readable")
            .is_some(),
        "close observes the already-activated rejection record"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_direct_rejection_shared_guard_fences_generation_replacement_until_activation() {
    const TERMINAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    let runtime = runtime();
    let transaction = TransactionBuilder::default().version(1u32).build();
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture has one Direct execution slot");
    let direct = BoundedTransaction::try_new(transaction)
        .expect("the stable rejection fixture is bounded")
        .into_direct();
    let AuthorityDirectResolutionOutcome::Rejected(rejection) = runtime
        .resolve_local_transaction(&direct, execution)
        .expect("the stable rejection is typed")
    else {
        panic!("the non-zero transaction version rejects before resolution")
    };
    let (probe, staged, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::DirectRejectionEffectStagedBeforeReadCut,
            Some(probe),
        );
    });
    let commit_runtime = runtime.clone();
    let (terminal_tx, terminal_rx) = std::sync::mpsc::sync_channel(1);
    let terminal = std::thread::spawn(move || {
        terminal_tx
            .send(commit_runtime.settle_direct_transaction_rejection(rejection))
            .expect("the Direct terminal observer remains alive");
    });
    staged
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("the Direct terminal is staged under its shared outer guard");

    let replacement_runtime = runtime.clone();
    let replacement = tokio::spawn(async move {
        replacement_runtime
            .replace_current_generation_after_allocation()
            .await
    });
    tokio::time::timeout(TERMINAL_TIMEOUT, async {
        loop {
            if runtime.lifecycle_fence.state.lock().lifecycle_writer_active {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("generation replacement reaches the outer writer boundary");
    assert!(
        !replacement.is_finished(),
        "generation replacement cannot pass the Direct terminal's shared guard"
    );
    release.send(()).expect("release Direct effect activation");
    assert!(matches!(
        terminal_rx
            .recv_timeout(TERMINAL_TIMEOUT)
            .expect("the Direct terminal returns before replacement"),
        Ok(AuthorityDirectRejectionExecution::Local(_))
    ));
    terminal.join().expect("the Direct terminal does not panic");
    replacement
        .await
        .expect("the replacement task remains healthy")
        .expect("generation replacement follows the completed Direct terminal");
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::DirectRejectionEffectStagedBeforeReadCut,
            None,
        );
    });
}

#[tokio::test]
async fn runtime_effect_close_wakes_an_idle_level_waiter() {
    let runtime = runtime();
    let waiter = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.wait_effect_publication_for_foundation().await })
    };
    tokio::task::yield_now().await;

    runtime
        .close_effects()
        .await
        .expect("an idle authority closes without a synthetic effect");
    let publication = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("close cannot lose the idle publisher wake")
        .expect("the publisher task remains healthy");
    assert!(publication.is_none());
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
fn runtime_configuration_reserves_parent_progress_on_the_exact_executor() {
    let consensus = ConsensusBuilder::default().build();
    for (runtime_workers, verify_workers, expected_permits, expected_mode) in [
        (1, 1, 1, TxPoolVmExecutionMode::YieldRuntimeWorker),
        (2, 1, 1, TxPoolVmExecutionMode::Inline),
        (4, 3, 3, TxPoolVmExecutionMode::Inline),
        (8, 6, 7, TxPoolVmExecutionMode::Inline),
        (8, 1, 2, TxPoolVmExecutionMode::Inline),
    ] {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(runtime_workers)
            .enable_all()
            .build()
            .expect("the executor-shape fixture must build");
        let handle = Handle::new(runtime.handle().clone(), None);
        let mut config = runtime_config();
        config.max_tx_verify_workers = verify_workers;
        let compiled =
            AuthorityRuntimeConfig::from_runtime_with_handle(&config, &consensus, &handle)
                .expect("the executor-bound authority configuration must compile");
        assert_eq!(
            compiled.executor_shape_for_test(),
            (expected_permits, expected_mode),
            "runtime_workers={runtime_workers}, verify_workers={verify_workers}"
        );
    }
}

#[test]
fn runtime_configuration_rejects_a_current_thread_executor() {
    let consensus = ConsensusBuilder::default().build();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the current-thread negative fixture must build");
    let handle = Handle::new(runtime.handle().clone(), None);
    assert_eq!(
        AuthorityRuntimeConfig::from_runtime_with_handle(&runtime_config(), &consensus, &handle,)
            .err(),
        Some(RuntimeConfigError::ResourceConfiguration)
    );
}

#[test]
fn runtime_configuration_rejects_an_unusable_verification_time_policy() {
    let consensus = ConsensusBuilder::default().build();
    for config in [
        {
            let mut config = runtime_config();
            config.tx_verify_cycles_per_ms = 0;
            config
        },
        {
            let mut config = runtime_config();
            config.min_tx_verify_time_ms = 0;
            config
        },
        {
            let mut config = runtime_config();
            config.min_tx_verify_time_ms = 30_001;
            config.max_tx_verify_time_ms = 30_000;
            config
        },
        {
            let mut config = runtime_config();
            config.max_tx_verify_initial_load_bytes = 0;
            config
        },
    ] {
        assert_eq!(
            AuthorityRuntimeConfig::from_runtime(&config, &consensus).err(),
            Some(RuntimeConfigError::VerificationTimeConfiguration)
        );
    }
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
