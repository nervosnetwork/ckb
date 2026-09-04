use super::{
    AuthorityComputeOutcome, AuthorityDirectRejectionExecution, AuthorityDirectResolutionOutcome,
    AuthorityDriverError, AuthorityReadyCommitAssignment, AuthorityReadyCommitLane,
    AuthorityReadyCommitTerminal, AuthorityReadyDispatch, AuthorityReadyOutcome, AuthorityRuntime,
    AuthorityRuntimeConfig, FinalAdmissionCaptureError, PREACCEPTED_ENTRY_BYTES, PlanError,
    RetainedIngressBatchFailureReason, RuntimeConfigError, SettlementOrigin,
};
use crate::authority::effect::{
    CommittedEffect, CommittedRejection, EffectBatchBound, EffectBatchBounds, EffectCapacity,
    EffectLimits, EffectPolicy, RejectionAudience,
};
use crate::authority::ingress::{
    BoundedTransaction, RetainedAdmissionBatch, RetainedIngressAttempt, proposal,
    test_support::remote_at_for_foundation,
};
use crate::authority::plan::{
    Backpressure, CompiledSharedIndependent, SharedReadyWaveCompilation, StalePlan, TxPoolAuthority,
};
use crate::authority::scheduler::ReadyReservation;
use crate::authority::shard::{ConcurrentRemovalProbe, SharedIngressProbePhase};
use crate::authority::state::{
    OwnedTx, PreAcceptedPhase, RawTxHash, ValidatedAdmission, VerifyCapability, WorkPermit,
    test_support::RejectionKind,
};
use crate::authority::tests::foundation::{independent_batch, leaf_rbf_pair};
use crate::authority::worker::{AuthorityWorkerRole, test_support::AuthorityTestWorkerOwner};
use crate::constants::MAX_READY_BATCH;
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
    prelude::Pack,
};
use std::ops::ControlFlow;
use std::sync::{Arc, atomic::AtomicUsize};
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

fn continued<T>(flow: ControlFlow<super::AuthorityPendingSettlement, T>) -> T {
    match flow {
        ControlFlow::Continue(value) => value,
        ControlFlow::Break(_) => panic!("the fixture has sufficient effect capacity"),
    }
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

fn compile_ready_singleton_for_test(
    authority: &TxPoolAuthority,
    hash: &RawTxHash,
) -> CompiledSharedIndependent {
    let batch = independent_batch(authority, std::slice::from_ref(hash));
    match authority.compile_shared_ready_wave(&batch) {
        SharedReadyWaveCompilation::Complete(mut compiled) if compiled.len() == 1 => compiled
            .pop()
            .expect("a one-member production wave has one sealed job"),
        SharedReadyWaveCompilation::Complete(compiled)
        | SharedReadyWaveCompilation::Error { compiled, .. } => {
            for candidate in compiled {
                let _ = candidate.cancel_unassigned_ready_job();
            }
            panic!("the Ready singleton did not compile into exactly one production job")
        }
        SharedReadyWaveCompilation::Prefix(prefix) => {
            let (compiled, boundary) = prefix.into_parts();
            for candidate in compiled {
                let _ = candidate.cancel_unassigned_ready_job();
            }
            let _ = boundary.cancel_unassigned_ready_job();
            panic!("one Ready candidate cannot have an incompatible prefix boundary")
        }
        SharedReadyWaveCompilation::Retry => panic!("the Ready singleton unexpectedly staled"),
        SharedReadyWaveCompilation::EffectCapacity => {
            panic!("the Ready singleton unexpectedly exhausted effect capacity")
        }
    }
}

fn reserve_excluding_one_compatible_ready_wave(
    runtime: &AuthorityRuntime,
    hashes: &[RawTxHash],
) -> ([RawTxHash; 2], ReadyReservation) {
    runtime.with_authority_for_foundation(|authority| {
        let mut selected = None;
        'left: for left_index in 0..hashes.len() {
            let left = compile_ready_singleton_for_test(authority, &hashes[left_index]);
            let left_support = left.physical_apply_support_for_foundation();
            drop(left);
            for (right_index, right_hash) in
                hashes.iter().enumerate().skip(left_index.saturating_add(1))
            {
                let right = compile_ready_singleton_for_test(authority, right_hash);
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

fn reserve_excluding_one_compatible_cross_class_ready_wave(
    runtime: &AuthorityRuntime,
    remote_hashes: &[RawTxHash],
    trusted_hashes: &[RawTxHash],
) -> [RawTxHash; 2] {
    let selected = runtime.with_authority_for_foundation(|authority| {
        let mut selected = None;
        'remote: for remote in remote_hashes {
            let remote_compiled = compile_ready_singleton_for_test(authority, remote);
            for trusted in trusted_hashes {
                let trusted_compiled = compile_ready_singleton_for_test(authority, trusted);
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
                .settle_effect_for_foundation(receipt.complete_for_foundation())
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
    let assignments = wave.into_assignments();
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
    for assignment in assignments {
        assert!(matches!(
            runtime.commit_ready_assignment(assignment),
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
            .settle_effect_for_foundation(receipt.complete_for_foundation())
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
async fn runtime_ready_slot_stays_reserved_through_effect_rollback_terminal() {
    const TERMINAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let mut hashes = Vec::new();
    hashes
        .try_reserve_exact(8)
        .expect("the bounded rollback-order fixture allocates");
    for offset in 0..8usize {
        hashes.push(advance_remote_to_ready(&runtime, 1_175 + offset as u32, 1_175 + offset).await);
    }
    let (selected, excluded_reservation) =
        reserve_excluding_one_compatible_ready_wave(&runtime, &hashes);
    let dispatch = runtime
        .try_drive_ready()
        .expect("the compatible Ready pair compiles into production assignments");
    let AuthorityReadyDispatch::Wave(wave) = dispatch else {
        panic!("the compatible Ready pair must dispatch one shared wave");
    };
    let mut assignments = wave.into_assignments();
    assert_eq!(assignments.len(), selected.len());
    let held = assignments
        .pop()
        .expect("the second Ready assignment remains reserved");
    let cancelled = assignments
        .pop()
        .expect("the first Ready assignment reaches rollback");

    let capacity_wake = runtime.effect_capacity_signal().notified();
    tokio::pin!(capacity_wake);
    let _ = capacity_wake.as_mut().enable();
    let (probe, rollback_terminal, release_terminal) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.set_staged_rollback_terminal_probe_for_foundation(Some(probe));
    });
    let cancel_runtime = runtime.clone();
    let cancel = std::thread::spawn(move || cancel_runtime.cancel_ready_assignment(cancelled));
    rollback_terminal
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("the real staged effect reaches its rollback terminal");

    runtime.with_authority_for_foundation(|authority| {
        assert_eq!(authority.ready_reserved_len_for_foundation(), hashes.len());
        assert!(
            authority
                .reserve_ready_candidates()
                .expect("the Ready projection remains valid")
                .is_none(),
            "rollback cannot expose Ready before its caller releases the slot capability"
        );
    });
    release_terminal
        .send(())
        .expect("release the terminalized rollback caller");
    cancel
        .join()
        .expect("the Ready cancellation thread remains healthy")
        .expect("the production cancellation returns its exact wake");
    tokio::time::timeout(TERMINAL_TIMEOUT, capacity_wake.as_mut())
        .await
        .expect("the returned rollback wake releases effect capacity");

    let recaptured = runtime.with_authority_for_foundation(|authority| {
        let recaptured = authority
            .reserve_ready_candidates()
            .expect("the post-terminal Ready projection remains valid")
            .expect("the cancelled Ready slot becomes capturable after capability release");
        let recaptured_hashes = recaptured
            .candidates()
            .map(|(hash, _)| hash.clone())
            .collect::<Vec<_>>();
        assert_eq!(recaptured_hashes.len(), 1);
        assert!(selected.contains(&recaptured_hashes[0]));
        recaptured
    });
    runtime
        .cancel_ready_assignment(held)
        .expect("the held Ready assignment terminalizes exactly");
    drop(recaptured);
    drop(excluded_reservation);
    runtime.with_authority_for_foundation(|authority| {
        let reap = authority
            .reserve_ready_candidates()
            .expect("the terminal claims remain reapable");
        drop(reap);
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
        assert!(authority.primary_projection_consistent());
    });
    let effects = runtime.effect_observation_for_foundation();
    assert!(effects.queued.is_empty());
    assert!(effects.blocking_staged_head.is_none());
    assert_eq!(
        (effects.total_usage.batches, effects.total_usage.bytes),
        (0, 0)
    );
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
    let assignments = wave.into_assignments();
    assert_eq!(assignments.len(), 1);
    for assignment in assignments {
        assert!(matches!(
            runtime.commit_ready_assignment(assignment),
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
async fn runtime_stale_ready_rollback_releases_store_read_before_effect_terminal() {
    const EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let runtime = runtime();
    let hash = advance_remote_to_ready(&runtime, 1_350, 1_350).await;
    let assignment = runtime.with_authority_for_foundation(|authority| {
        let compiled = compile_ready_singleton_for_test(authority, &hash);
        let reservation = authority.reserve_ready_exact_for_foundation(std::slice::from_ref(&hash));
        let (mut slots, remainder) = reservation
            .try_split_prefix(1)
            .unwrap_or_else(|_| panic!("the exact Ready singleton splits into one worker slot"));
        assert!(remainder.is_none());
        AuthorityReadyCommitAssignment {
            compiled,
            reservation: slots.pop().expect("the singleton owns one Ready slot"),
        }
    });
    runtime
        .clear_pool(genesis_snapshot())
        .await
        .expect("generation replacement makes the compiled assignment stale");

    let (probe, rollback_terminal, release_terminal) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.set_staged_rollback_terminal_probe_for_foundation(Some(probe));
    });
    let commit_runtime = runtime.clone();
    let commit = std::thread::spawn(move || commit_runtime.commit_ready_assignment(assignment));
    rollback_terminal
        .recv_timeout(EVENT_TIMEOUT)
        .expect("the stale assignment reaches its real effect rollback terminal");
    let write_available = runtime.generation_write_available_for_foundation();
    release_terminal
        .send(())
        .expect("release the stale rollback terminal");
    assert!(matches!(
        commit.join().expect("the stale Ready commit thread joins"),
        AuthorityReadyCommitTerminal::Stale
    ));
    runtime.with_authority_for_foundation(|authority| {
        authority.set_staged_rollback_terminal_probe_for_foundation(None);
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
        assert!(authority.primary_projection_consistent());
    });
    assert!(
        write_available,
        "effect rollback and Ready capability return must run after the global store read guard is released"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_ready_priority_stale_precedes_resource_capacity_begin() {
    let runtime = runtime();
    let weaker = advance_remote_to_ready(&runtime, 1_198, 1_198).await;
    let dispatch = runtime
        .try_drive_ready()
        .expect("the weaker Ready owner compiles through the production route");
    let AuthorityReadyDispatch::Wave(wave) = dispatch else {
        panic!("one ordinary Ready candidate dispatches one singleton job");
    };
    let mut assignments = wave.into_assignments();
    assert_eq!(assignments.len(), 1);
    let assignment = assignments
        .pop()
        .expect("the production wave owns one Ready capability");

    let stronger_admission =
        ValidatedAdmission::proposal(TransactionBuilder::default().version(1_199u32).build())
            .expect("the trusted interposition has valid ingress evidence");
    let stronger = advance_admission_to_ready(&runtime, stronger_admission).await;
    assert!(matches!(
        runtime.commit_ready_assignment(assignment),
        AuthorityReadyCommitTerminal::Stale
    ));

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
        assert!(!authority.resources().capacity_faulted_for_foundation());
        assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
        assert!(authority.primary_projection_consistent());
    });
    let effects = runtime.effect_observation_for_foundation();
    assert!(effects.queued.is_empty());
    assert!(effects.blocking_staged_head.is_none());
    assert_eq!(
        (effects.total_usage.batches, effects.total_usage.bytes),
        (0, 0)
    );
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
        .settle_effect_for_foundation(occupied_lease.complete_for_foundation())
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
    let replacement =
        tokio::spawn(async move { replacement_runtime.clear_pool(genesis_snapshot()).await });
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
fn ready_owner_staleness_between_scheduler_and_owner_cuts_is_retryable() {
    let stale = FinalAdmissionCaptureError::Plan(PlanError::Stale(StalePlan::Version));
    assert_eq!(
        AuthorityDriverError::from_initial_ready_capture(stale),
        AuthorityDriverError::Stale
    );
    let stale = FinalAdmissionCaptureError::Plan(PlanError::Stale(StalePlan::Version));
    assert_eq!(
        AuthorityDriverError::from_ready_recheck(stale),
        AuthorityDriverError::Stale
    );
}

#[test]
fn runtime_rbf_same_version_source_change_during_dependency_plan_is_coherent() {
    const EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

    let snapshot = genesis_snapshot();
    let mut config = runtime_config();
    config.min_rbf_rate = FeeRate::from_u64(1_000);
    let runtime = AuthorityRuntime::new(
        &config,
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the replacement-enabled runtime fixture is valid");
    let (_victim, candidate) =
        runtime.with_authority_for_foundation(|authority| leaf_rbf_pair(authority, 191));
    let (candidate_tx, version) = runtime.with_authority_read_for_foundation(|authority| {
        let owner = authority
            .entry(&candidate)
            .expect("the Ready candidate exists");
        (owner.record().tx.as_ref().clone(), owner.record().version)
    });

    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_membership_dependency_plan_probe(Some(probe));
    });
    let driver_runtime = runtime.clone();
    let driver = std::thread::spawn(move || driver_runtime.try_drive_ready());
    entered
        .recv_timeout(EVENT_TIMEOUT)
        .expect("the planner reaches the final-cut boundary");

    let attempt = proposal(
        BoundedTransaction::try_new(candidate_tx).expect("the Proposal fixture is bounded"),
        snapshot.consensus(),
    );
    let batch = RetainedAdmissionBatch::new(attempt, std::collections::VecDeque::new())
        .expect("one Proposal attempt is a homogeneous batch");
    let (consumed, remaining, post_commit_fault) = runtime
        .commit_retained_ingress_batch(batch)
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("the same-witness source promotion commits")
        });
    assert_eq!(consumed, 1);
    assert!(remaining.is_empty());
    assert_eq!(post_commit_fault, None);
    assert_eq!(
        runtime.with_authority_read_for_foundation(|authority| {
            authority
                .entry(&candidate)
                .expect("the promoted candidate remains owned")
                .record()
                .version
        }),
        version,
        "the source-only promotion preserves EntryVersion"
    );

    release
        .send(())
        .expect("the planner may acquire its final cut");
    assert!(matches!(
        driver.join().expect("the Ready driver thread joins"),
        Ok(AuthorityReadyDispatch::Outcome(
            AuthorityReadyOutcome::Applied
        ))
    ));
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_membership_dependency_plan_probe(None);
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn runtime_rbf_dependency_planning_does_not_hold_an_owner_cut() {
    const EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

    let snapshot = genesis_snapshot();
    let mut config = runtime_config();
    config.min_rbf_rate = FeeRate::from_u64(1_000);
    let runtime = AuthorityRuntime::new(
        &config,
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the replacement-enabled runtime fixture is valid");
    let (victim, _candidate) =
        runtime.with_authority_for_foundation(|authority| leaf_rbf_pair(authority, 190));

    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_membership_dependency_plan_probe(Some(probe));
    });
    let driver_runtime = runtime.clone();
    let driver = std::thread::spawn(move || driver_runtime.try_drive_ready());
    entered
        .recv_timeout(EVENT_TIMEOUT)
        .expect("the planner reaches the membership dependency seam");
    let (writer_completed, writer_result) = std::sync::mpsc::channel();
    let writer_runtime = runtime.clone();
    let victim_hash = victim.0;
    let writer = std::thread::spawn(move || {
        let result = writer_runtime.remove_local_transaction(&victim_hash);
        let _ = writer_completed.send(result);
    });
    let write_result = writer_result.recv_timeout(EVENT_TIMEOUT);
    release
        .send(())
        .expect("the membership dependency planner resumes");
    assert_eq!(
        write_result.expect("dependency planning must not retain an owner-shard read cut"),
        Ok(true)
    );
    writer.join().expect("the exact-shard writer thread joins");
    let dispatch = driver
        .join()
        .expect("the Ready driver thread joins")
        .expect("the released dependency plan returns a retryable outcome");
    if let AuthorityReadyDispatch::Wave(wave) = dispatch {
        for assignment in wave.into_assignments() {
            assert!(matches!(
                runtime.commit_ready_assignment(assignment),
                AuthorityReadyCommitTerminal::Stale
            ));
        }
    }
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_membership_dependency_plan_probe(None);
        assert!(authority.primary_projection_consistent());
    });
}
