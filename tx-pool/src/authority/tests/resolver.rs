use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, apply_plan,
    direct_verified_facts_for_view, limits, owner_version, resolved_payload_with_facts,
    runtime_config, tx,
};
use crate::authority::{
    ingress::{
        BoundedTransaction, DirectCommand, DirectIngressTransaction, RetainedAdmissionBatch,
        direct, proposal,
    },
    plan::TxPoolAuthority,
    resolver::{
        DirectResolutionEvaluation, DirectResolutionJob, DirectResolutionPreparation,
        DirectResolutionProbeObservation, ResolutionEvaluation, ResolutionExecutionKind,
        ResolutionJob, VerificationExecution, VerificationJob, VerificationTimePolicy,
    },
    resources::{AcceptedResources, ComputeLimits, ResourceLimits, ResourceVector},
    runtime::{
        AuthorityDirectAdmissionError, AuthorityDirectAdmissionExecution,
        AuthorityDirectRejectionExecution, AuthorityDirectResolutionOutcome,
        AuthorityDirectVerificationOutcome, AuthorityDirectVerifiedCandidate,
        AuthorityLocalAdmissionOutcome, AuthorityRuntime, AuthorityTestAcceptOutcome,
        DirectAdmissionRejectionKind,
    },
    shard::{ConcurrentRemovalProbe, SharedIngressProbePhase},
    state::{
        AcceptedProvenance, AcceptedStatus, ChainRevision, ChainViewId, DependencyKey, OwnedTx,
        PayloadPolicy, PreAcceptedPhase, QueuedWork, RawTxHash, ValidatedAdmission,
        VerifyCapability, WorkPermit,
    },
    work::{CheckedOutWork, SettlementNext},
};
use crate::error::Reject;
use ckb_network::PeerIndex;
use ckb_script::{ChunkCommand, TxVerifyEnv};
use ckb_snapshot::Snapshot;
use ckb_store::attach_block_cell;
use ckb_test_chain_utils::{
    MockStore, always_success_cell, always_success_consensus, create_always_success_out_point,
};
use ckb_types::{
    U256,
    bytes::Bytes,
    core::{BlockExt, Capacity, FeeRate, TransactionBuilder},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack, Unpack},
};
use ckb_verification::cache::{ScriptVerificationRules, TxVerificationCacheKey, init_cache};
use std::{ops::ControlFlow, sync::Arc};

fn verification_time_limit() -> std::time::Duration {
    std::time::Duration::from_secs(30)
}

fn initial_load_limit() -> ckb_script::InitialProgramLoadLimit {
    ckb_script::InitialProgramLoadLimit::new(u64::MAX)
        .expect("the test initial-load limit is non-zero")
}

fn verification_budget_with(
    active_vm_time: std::time::Duration,
) -> crate::util::TxPoolVerificationBudget {
    crate::util::TxPoolVerificationBudget::new(active_vm_time, initial_load_limit())
}

fn verification_budget() -> crate::util::TxPoolVerificationBudget {
    verification_budget_with(verification_time_limit())
}

#[test]
fn uak_verification_time_policy_is_fixed_bounded_and_never_peer_extended() {
    let policy = VerificationTimePolicy::from_runtime(250, 10_000, 30_000)
        .expect("the fixture policy is valid");
    assert_eq!(
        policy.duration(PayloadPolicy::remote_for_foundation(1)),
        std::time::Duration::from_millis(250),
        "a low peer declaration receives only the fixed minimum budget"
    );
    assert_eq!(
        policy.duration(PayloadPolicy::remote_for_foundation(2_500_001)),
        std::time::Duration::from_millis(251),
        "the cycle signal rounds up rather than truncating a partial millisecond"
    );
    let slow_signal_policy = VerificationTimePolicy::from_runtime(250, 1, 30_000)
        .expect("the slow-signal fixture policy is valid");
    assert_eq!(
        slow_signal_policy.duration(PayloadPolicy::remote_for_foundation(70_000_000)),
        std::time::Duration::from_secs(30),
        "an untrusted declaration can never extend the unconditional hard cap"
    );
    assert_eq!(
        policy.duration(PayloadPolicy::Trusted),
        std::time::Duration::from_secs(30),
        "trusted local/proposal work is constrained only by the node hard cap"
    );
}

fn direct_input(transaction: &ckb_types::core::TransactionView) -> DirectIngressTransaction {
    BoundedTransaction::try_new(transaction.clone())
        .expect("direct fixture transaction is bounded")
        .into_direct()
}

async fn verified_direct_candidate(
    runtime: &AuthorityRuntime,
    transaction: &ckb_types::core::TransactionView,
    command: DirectCommand,
) -> AuthorityDirectVerifiedCandidate {
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture has one free transient compute slot");
    let input = direct_input(transaction);
    let resolution = match command {
        DirectCommand::Local => runtime.resolve_local_transaction(&input, execution),
        DirectCommand::TestAccept => runtime.resolve_test_accept_transaction(&input, execution),
    };
    let request = match resolution.expect("direct resolution has sufficient host resources") {
        AuthorityDirectResolutionOutcome::Verification(request) => request,
        AuthorityDirectResolutionOutcome::Rejected(rejection) => panic!(
            "fixture transaction must reach verification: {:?}",
            rejection.reason().reject()
        ),
    };
    let cache = init_cache();
    let (_command_tx, mut command_rx) = tokio::sync::watch::channel(ChunkCommand::Resume);
    let outcome = runtime
        .execute_direct_verification(request.bind_cache(&cache), &mut command_rx)
        .await
        .expect("direct verification executes under the captured rules");
    match outcome {
        AuthorityDirectVerificationOutcome::Candidate(candidate) => candidate,
        AuthorityDirectVerificationOutcome::Rejected(rejection) => panic!(
            "fixture transaction must remain a verified candidate: {:?}",
            rejection.reason().reject()
        ),
    }
}

async fn verified_local_candidate(
    runtime: &AuthorityRuntime,
    transaction: &ckb_types::core::TransactionView,
) -> AuthorityDirectVerifiedCandidate {
    verified_direct_candidate(runtime, transaction, DirectCommand::Local).await
}

async fn verified_test_accept_candidate(
    runtime: &AuthorityRuntime,
    transaction: &ckb_types::core::TransactionView,
) -> AuthorityDirectVerifiedCandidate {
    verified_direct_candidate(runtime, transaction, DirectCommand::TestAccept).await
}

fn genesis_snapshot() -> Arc<Snapshot> {
    let consensus = Arc::new(always_success_consensus());
    let store = MockStore::default();
    let genesis = consensus.genesis_block();
    let epoch_ext = consensus.genesis_epoch_ext().clone();
    {
        let db_txn = store.store().begin_transaction();
        let previous_epoch_hash = epoch_ext.last_block_hash_in_previous_epoch();
        db_txn
            .insert_block(genesis)
            .expect("the fixture stores genesis");
        db_txn
            .attach_block(genesis)
            .expect("the fixture attaches genesis");
        attach_block_cell(&db_txn, genesis).expect("the fixture stores genesis cells");
        db_txn
            .insert_block_epoch_index(&genesis.hash(), &previous_epoch_hash)
            .expect("the fixture stores the epoch index");
        db_txn
            .insert_epoch_ext(&previous_epoch_hash, &epoch_ext)
            .expect("the fixture stores the epoch extension");
        db_txn
            .insert_block_ext(
                &genesis.hash(),
                &BlockExt {
                    received_at: 0,
                    total_difficulty: U256::zero(),
                    total_uncles_count: 0,
                    verified: Some(true),
                    txs_fees: vec![],
                    cycles: None,
                    txs_sizes: None,
                },
            )
            .expect("the fixture stores the block extension");
        db_txn.commit().expect("the fixture commits genesis");
    }
    Arc::new(Snapshot::new(
        genesis.header(),
        U256::zero(),
        epoch_ext,
        store.store().get_snapshot(),
        Default::default(),
        consensus,
    ))
}

fn authority_at(snapshot: &Snapshot) -> TxPoolAuthority {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    authority.force_chain_view(ChainViewId::new(ChainRevision(0), snapshot.tip_hash()));
    authority
}

fn output(capacity: u64) -> CellOutput {
    // Preserve fixture fee deltas while satisfying occupied-capacity verification.
    const FIXTURE_OUTPUT_BASE_SHANNONS: u64 = 100 * 100_000_000;
    CellOutput::new_builder()
        .capacity(
            Capacity::shannons(
                FIXTURE_OUTPUT_BASE_SHANNONS
                    .checked_add(capacity)
                    .expect("fixture capacity arithmetic is bounded"),
            )
            .pack(),
        )
        .lock(always_success_cell().2.clone())
        .build()
}

fn output_tx(version: u32, capacity: u64, data: Bytes) -> ckb_types::core::TransactionView {
    TransactionBuilder::default()
        .version(version)
        .output(output(capacity))
        .output_data(data.pack())
        .build()
}

fn spending_tx(
    version: u32,
    inputs: impl IntoIterator<Item = OutPoint>,
    capacity: u64,
) -> ckb_types::core::TransactionView {
    inputs
        .into_iter()
        .fold(
            TransactionBuilder::default().version(version),
            |builder, input| builder.input(CellInput::new(input, 0)),
        )
        .output(output(capacity))
        .output_data(Bytes::new().pack())
        .cell_dep(
            CellDep::new_builder()
                .out_point(create_always_success_out_point())
                .build(),
        )
        .build()
}

fn checkout_resolve(
    authority: &mut TxPoolAuthority,
    tx: ckb_types::core::TransactionView,
    peer: usize,
) -> super::super::work::ResolveWork {
    let admission = ValidatedAdmission::remote(tx, PeerIndex::from(peer))
        .expect("fixture ingress evidence is valid");
    let key = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("fixture ingress plans"),
    );
    let committed = authority
        .checkout_for_foundation(
            &key,
            owner_version(authority, &key),
            WorkPermit::ResolveOnly,
        )
        .expect("resolve checkout plans");
    let CheckedOutWork::Resolve(work) = committed.into_work() else {
        panic!("resolve-only permit must carry resolve work")
    };
    work
}

fn checkout_verification_job(
    authority: &mut TxPoolAuthority,
    snapshot: Arc<Snapshot>,
    tx: ckb_types::core::TransactionView,
    peer: usize,
) -> VerificationJob {
    let key = crate::authority::state::RawTxHash(tx.hash());
    let resolve = checkout_resolve(authority, tx, peer);
    let resolution = ResolutionJob::capture_resolve(authority, Arc::clone(&snapshot), resolve)
        .expect("the resolve checkout uses the paired snapshot")
        .evaluate(FeeRate::zero(), u64::MAX)
        .expect("the fixture resolution is valid");
    let ResolutionEvaluation::Settle(settlement) = resolution else {
        panic!("resolve-only work must enqueue verification")
    };
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("the resolved receipt settles"),
    );
    let checkout = authority
        .checkout_for_foundation(
            &key,
            owner_version(authority, &key),
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("verification checkout plans");
    let CheckedOutWork::Verify(work) = checkout.into_work() else {
        panic!("verify-only checkout must carry verification work")
    };
    VerificationJob::from_checkout(work, snapshot)
        .expect("verification remains on the resolve snapshot")
}

#[test]
fn uak_verify_checkout_requeues_resolution_from_an_old_chain_view_before_vm() {
    let old_snapshot = genesis_snapshot();
    let mut authority = authority_at(&old_snapshot);
    let transaction = TransactionBuilder::default().version(811u32).build();
    let key = crate::authority::state::RawTxHash(transaction.hash());
    let resolve = checkout_resolve(&mut authority, transaction, 811);
    let ResolutionEvaluation::Settle(settlement) =
        ResolutionJob::capture_resolve(&authority, Arc::clone(&old_snapshot), resolve)
            .expect("the original resolve checkout uses the paired snapshot")
            .evaluate(FeeRate::zero(), u64::MAX)
            .expect("the fixture resolution is valid")
    else {
        panic!("resolve-only work must enqueue verification")
    };
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("the old-view resolution settles as queued verification"),
    );

    let consensus = old_snapshot.cloned_consensus();
    let new_header = old_snapshot
        .tip_header()
        .as_advanced_builder()
        .nonce(1u128.pack())
        .build();
    let new_snapshot = Arc::new(Snapshot::new(
        new_header.clone(),
        U256::zero(),
        consensus.genesis_epoch_ext().clone(),
        MockStore::default().store().get_snapshot(),
        Default::default(),
        consensus,
    ));
    authority.force_chain_view(ChainViewId::new(ChainRevision(1), new_header.hash()));
    let checkout = authority
        .checkout_for_foundation(
            &key,
            owner_version(&authority, &key),
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("the stale queued verification can be checked out without a pool scan");
    let CheckedOutWork::Verify(work) = checkout.into_work() else {
        panic!("verify-only checkout must carry verification work")
    };
    let failure = VerificationJob::from_checkout(work, new_snapshot)
        .expect_err("old location evidence must be rejected before VM execution");
    assert_eq!(failure.kind(), ResolutionExecutionKind::StaleView);
    apply_plan(
        authority
            .apply_settlement(failure.into_settlement())
            .expect("the exact stale capability requeues for resolution"),
    );
    assert!(matches!(
        authority.entry(&key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
}

#[tokio::test(flavor = "multi_thread")]
async fn uak_verification_request_binds_environment_rules_and_witness_cache_key() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let tx = TransactionBuilder::default().version(812u32).build();
    let witness_hash: [u8; 32] = tx.witness_hash().unpack();
    let expected_rules = ScriptVerificationRules::from_env(
        snapshot.consensus(),
        &TxVerifyEnv::new_submit(snapshot.tip_header()),
    );
    let expected_key = TxVerificationCacheKey::from_transaction(&tx, expected_rules);
    let job = checkout_verification_job(&mut authority, Arc::clone(&snapshot), tx, 92);
    let request = job.prepare(verification_budget());
    assert_eq!(expected_key.witness_hash(), &witness_hash);
    let cache = init_cache();
    let (_command_tx, mut command_rx) = tokio::sync::watch::channel(ChunkCommand::Resume);

    let execution = request.bind_cache(&cache).execute(&mut command_rx).await;
    let VerificationExecution {
        settlement,
        cache_update,
    } = execution;
    let update = cache_update.expect("a real cache miss publishes VM-success evidence");
    assert_eq!(update.into_proof().key(), expected_key);
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("the exact verification capability settles"),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn uak_zero_active_budget_allows_verification_with_no_vm_slices() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let transaction = TransactionBuilder::default().version(8_120u32).build();
    let request =
        checkout_verification_job(&mut authority, Arc::clone(&snapshot), transaction, 9_120)
            .prepare(verification_budget_with(std::time::Duration::ZERO));
    let cache = init_cache();
    let (_command_tx, mut command_rx) = tokio::sync::watch::channel(ChunkCommand::Resume);

    let VerificationExecution {
        settlement,
        cache_update,
    } = request.bind_cache(&cache).execute(&mut command_rx).await;
    assert!(
        cache_update.is_some(),
        "context checks and a transaction with no VM groups consume no active budget"
    );
    assert!(
        matches!(&settlement.next, SettlementNext::Ready(_)),
        "zero active slices must still produce Ready: {:?}",
        settlement.next
    );
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("the exact zero-slice verification capability settles once"),
    );
    assert!(authority.primary_projection_consistent());
}

#[tokio::test(flavor = "multi_thread")]
async fn uak_initial_program_load_limit_is_transient_and_never_cached() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let parent = output_tx(8_122, 1_000, Bytes::new());
    accept_remote_transaction(
        &mut authority,
        parent.clone(),
        9_121,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let transaction = spending_tx(8_123, [OutPoint::new(parent.hash(), 0)], 900);
    let request =
        checkout_verification_job(&mut authority, Arc::clone(&snapshot), transaction, 9_123)
            .prepare(crate::util::TxPoolVerificationBudget::new(
                verification_time_limit(),
                ckb_script::InitialProgramLoadLimit::new(1)
                    .expect("the rejecting fixture limit is non-zero"),
            ));
    let cache = init_cache();
    let (_command_tx, mut command_rx) = tokio::sync::watch::channel(ChunkCommand::Resume);

    let VerificationExecution {
        settlement,
        cache_update,
    } = request.bind_cache(&cache).execute(&mut command_rx).await;
    assert!(
        cache_update.is_none(),
        "a local load refusal is never VM proof"
    );
    assert!(
        matches!(
            &settlement.next,
            SettlementNext::VerificationRejected { rejection, .. }
                if matches!(rejection.reject(), Reject::Full(_))
                    && !rejection.should_record()
                    && rejection.publish_negative_relay_terminal()
        ),
        "unexpected initial-load settlement: {:?}",
        settlement.next
    );
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("the exact load-refused verification capability settles once"),
    );
    assert!(authority.primary_projection_consistent());
}

#[tokio::test(flavor = "multi_thread")]
async fn uak_verification_cache_lookup_cannot_substitute_a_nearby_request() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let cached_tx = TransactionBuilder::default().version(813u32).build();
    let requested_tx = TransactionBuilder::default().version(814u32).build();
    let mut cache = init_cache();
    let (_command_tx, mut command_rx) = tokio::sync::watch::channel(ChunkCommand::Resume);
    let cached = checkout_verification_job(&mut authority, Arc::clone(&snapshot), cached_tx, 93)
        .prepare(verification_budget())
        .bind_cache(&cache)
        .execute(&mut command_rx)
        .await;
    let cached_proof = cached
        .cache_update
        .expect("the nearby fixture proof must originate in a real VM success")
        .into_proof();
    cache.insert(cached_proof);
    apply_plan(
        authority
            .apply_settlement(cached.settlement)
            .expect("the nearby fixture capability settles"),
    );
    let request =
        checkout_verification_job(&mut authority, Arc::clone(&snapshot), requested_tx, 94)
            .prepare(verification_budget());

    let execution = request.bind_cache(&cache).execute(&mut command_rx).await;
    assert!(execution.cache_update.is_some());
    apply_plan(
        authority
            .apply_settlement(execution.settlement)
            .expect("the cache-miss verification capability settles"),
    );
}

#[test]
fn uak_direct_edge_budget_is_a_policy_rejection_not_a_runtime_fault() {
    let snapshot = genesis_snapshot();
    let input = OutPoint::new(Byte32::new([0xb3; 32]), 0);
    let transaction = spending_tx(0, [input], 1);
    let direct_input = direct_input(&transaction);
    let direct = direct(
        &direct_input,
        snapshot.consensus(),
        DirectCommand::TestAccept,
    )
    .expect("the transaction passes non-contextual validation");
    let DirectResolutionPreparation::Rejected(rejection) =
        DirectResolutionJob::prepare(direct, 1 << 20, 0)
            .expect("hostile edge count is an ordinary preparation outcome")
    else {
        panic!("the zero-edge envelope must reject before resolution")
    };
    assert!(matches!(
        rejection.reason().reject(),
        crate::error::Reject::Full(_)
    ));
}

#[test]
fn uak_direct_permissive_rbf_never_fabricates_a_chain_cell() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let parent = output_tx(816, 1_000, Bytes::new());
    let parent_out = OutPoint::new(parent.hash(), 0);
    accept_remote_transaction(
        &mut authority,
        parent,
        94,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    accept_remote_transaction(
        &mut authority,
        spending_tx(817, [parent_out.clone()], 900),
        95,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let unknown = OutPoint::new(Byte32::new([0xb2; 32]), 0);
    let replacement = Arc::new(spending_tx(818, [unknown.clone(), parent_out], 800));
    let before = authority.normalized_snapshot();
    let job = DirectResolutionJob::capture_for_foundation(
        &authority,
        Arc::clone(&snapshot),
        replacement,
        1 << 20,
        1_000,
    )
    .expect("the direct RBF cut captures the accepted spender");
    let DirectResolutionEvaluation::Enrich(probe) = job
        .evaluate(FeeRate::zero(), verification_budget())
        .expect("permissive RBF still consults the chain snapshot")
    else {
        panic!("the unknown chain input cannot become resolved evidence")
    };
    assert_eq!(
        probe.missing_keys_for_foundation(),
        vec![DependencyKey::Cell(unknown.clone())]
    );
    let DirectResolutionProbeObservation::Rejected(rejection) = probe
        .prepare_enrichment()
        .expect("the bounded missing frontier is reservable")
        .observe(&authority)
        .finish()
        .expect("the paired authority view remains current")
    else {
        panic!("no accepted producer can satisfy the unknown chain input")
    };
    assert!(matches!(
        rejection.reason().reject(),
        crate::error::Reject::Resolve(ckb_types::core::error::OutPointError::Unknown(out_point))
            if out_point == &unknown
    ));
    assert_eq!(authority.normalized_snapshot(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_direct_resource_contention_wakes_and_retries_the_same_bank() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let transaction = Arc::new(tx(860));
    let chain_view =
        runtime.with_authority_for_foundation(|authority| authority.chain_view().clone());
    let verified = || {
        direct_verified_facts_for_view(
            &transaction,
            chain_view.clone(),
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1_000),
        )
    };
    let candidate = runtime.direct_candidate_for_foundation(Arc::clone(&transaction), verified());
    let wait = runtime.resource_capacity_wait_identity();
    let signal = wait.terminal_signal();
    let notified = signal.notified();
    tokio::pin!(notified);
    let _ = notified.as_mut().enable();
    let held = runtime
        .with_authority_for_foundation(|authority| {
            authority.hold_positive_accepted_reservation_for_foundation()
        })
        .expect("the sibling plan reserves the exact Accepted capacity");
    let AuthorityDirectAdmissionError::ResourceContended(actual) = runtime
        .settle_verified_direct_admission(candidate)
        .expect_err("a live sibling reservation is typed contention, not rejection")
    else {
        panic!("the sibling reservation must retain its exact wait identity")
    };
    assert!(actual.same_bank(&wait));
    held.release();
    tokio::time::timeout(std::time::Duration::from_secs(2), notified.as_mut())
        .await
        .expect("the reservation terminal wakes the pre-enabled same-bank waiter");

    let retry = runtime.direct_candidate_for_foundation(Arc::clone(&transaction), verified());
    let AuthorityDirectAdmissionExecution::Local(execution) = runtime
        .settle_verified_direct_admission(retry)
        .expect("the same transaction accepts after the reservation terminal")
    else {
        panic!("the Local source retains its mutation semantics")
    };
    assert!(matches!(
        execution.into_parts().0,
        AuthorityLocalAdmissionOutcome::Accepted(_)
    ));
}

#[test]
fn uak_concurrent_direct_rbf_resource_projection_drift_is_stale() {
    const TERMINAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let snapshot = genesis_snapshot();
    let mut config = runtime_config();
    config.min_rbf_rate = FeeRate::from_u64(1_000);
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the replacement-enabled runtime fixture is valid");
    let input = OutPoint::new(Byte32::new([0xb4; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(30_000u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction_with_payload(
            authority,
            victim_tx.clone(),
            96,
            AcceptedStatus::Pending,
            resolved_payload_with_facts(
                &victim_tx,
                Vec::new(),
                vec![input.clone()],
                Capacity::shannons(100),
            ),
        );
    });
    let view = runtime.with_authority_for_foundation(|authority| authority.chain_view().clone());
    let candidate = |version: u32, fee: u64| {
        let transaction = Arc::new(
            TransactionBuilder::default()
                .version(version)
                .input(CellInput::new(input.clone(), 0))
                .build(),
        );
        (
            runtime.direct_candidate_for_foundation(
                Arc::clone(&transaction),
                direct_verified_facts_for_view(
                    &transaction,
                    view.clone(),
                    Vec::new(),
                    vec![input.clone()],
                    Capacity::shannons(fee),
                ),
            ),
            RawTxHash(transaction.hash()),
        )
    };
    let (first, _) = candidate(30_001, 30_000);
    let (second, second_hash) = candidate(30_002, 40_000);

    let (probe, before_resource_plan, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::DirectMembershipBeforeResourcePlan,
            Some(probe),
        );
    });
    let first_runtime = runtime.clone();
    let (first_tx, first_rx) = std::sync::mpsc::channel();
    let first_thread = std::thread::spawn(move || {
        let _ = first_tx.send(first_runtime.settle_verified_direct_admission(first));
    });
    before_resource_plan
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("the first RBF compiler reaches its resource seam");
    runtime.with_authority_read_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::DirectMembershipBeforeResourcePlan,
            None,
        );
    });

    let second_result = runtime.settle_verified_direct_admission(second);
    release
        .send(())
        .expect("release the stale first RBF compiler");
    let first_result = first_rx
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("the first RBF compiler returns after release");
    first_thread
        .join()
        .expect("the first RBF compiler does not panic");

    let Ok(AuthorityDirectAdmissionExecution::Local(second_execution)) = second_result else {
        panic!("the second RBF candidate must commit while the first compiler is paused")
    };
    assert!(matches!(
        second_execution.into_parts().0,
        AuthorityLocalAdmissionOutcome::Accepted(_)
    ));
    let Ok(AuthorityDirectAdmissionExecution::Local(first_execution)) = first_result else {
        panic!("concurrent resource drift must be a local retry")
    };
    assert!(matches!(
        first_execution.into_parts().0,
        AuthorityLocalAdmissionOutcome::Retry(_)
    ));
    runtime.with_authority_for_foundation(|authority| {
        assert!(authority.primary_projection_consistent());
        assert!(matches!(
            authority.entry(&second_hash),
            Some(OwnedTx::Accepted(_))
        ));
    });
}

fn while_outer_reader_is_held<T: Send>(
    runtime: &AuthorityRuntime,
    timeout: std::time::Duration,
    operation: impl FnOnce(AuthorityRuntime) -> T + Send,
) -> T {
    let (reader_entered_tx, reader_entered_rx) = std::sync::mpsc::channel();
    let (release_reader_tx, release_reader_rx) = std::sync::mpsc::channel();
    let (terminal_tx, terminal_rx) = std::sync::mpsc::channel();
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
            .expect("the independent outer reader is held");
        let commit_runtime = runtime.clone();
        let commit = scope.spawn(move || {
            let _ = terminal_tx.send(operation(commit_runtime));
        });
        let terminal = terminal_rx.recv_timeout(timeout);
        release_reader_tx
            .send(())
            .expect("release the independent outer reader");
        reader.join().expect("the outer reader does not panic");
        commit.join().expect("the Direct worker does not panic");
        terminal.expect("the operation cannot require the unrelated outer writer")
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_production_direct_acceptance_fences_generation_replacement_until_commit() {
    const TERMINAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let transaction = Arc::new(tx(871));
    let view = runtime.with_authority_for_foundation(|authority| authority.chain_view().clone());
    let candidate = runtime.direct_candidate_for_foundation(
        Arc::clone(&transaction),
        direct_verified_facts_for_view(
            &transaction,
            view,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1_000),
        ),
    );
    let (probe, prepared, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::DirectMembershipPreparedBeforeFinalCut,
            Some(probe),
        );
    });
    let commit_runtime = runtime.clone();
    let (terminal_tx, terminal_rx) = std::sync::mpsc::sync_channel(1);
    let terminal = std::thread::spawn(move || {
        terminal_tx
            .send(commit_runtime.settle_verified_direct_admission(candidate))
            .expect("the Direct terminal observer remains alive");
    });
    prepared
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("the Direct candidate is prepared under its shared generation barrier");

    let replacement_runtime = runtime.clone();
    let replacement_snapshot = Arc::clone(&snapshot);
    let replacement =
        tokio::spawn(async move { replacement_runtime.clear_pool(replacement_snapshot).await });
    tokio::time::timeout(TERMINAL_TIMEOUT, async {
        while !runtime.lifecycle_writer_active_for_foundation() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("generation replacement reaches the outer writer boundary");
    assert!(
        !replacement.is_finished(),
        "generation replacement cannot cross a prepared Direct acceptance"
    );

    release.send(()).expect("release the Direct final cut");
    let AuthorityDirectAdmissionExecution::Local(execution) = terminal_rx
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("Direct acceptance returns before replacement")
        .expect("the Direct acceptance remains healthy")
    else {
        panic!("the Local source retains mutation semantics")
    };
    assert!(matches!(
        execution.into_parts().0,
        AuthorityLocalAdmissionOutcome::Accepted(_)
    ));
    terminal.join().expect("the Direct terminal does not panic");
    replacement
        .await
        .expect("the replacement task remains healthy")
        .expect("generation replacement follows the committed Direct terminal");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_production_direct_duplicate_holds_its_owner_read_cut_through_effect_activation() {
    const TERMINAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let transaction = Arc::new(tx(868));
    let hash = runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(
            authority,
            transaction.as_ref().clone(),
            868,
            AcceptedStatus::Pending,
            Vec::new(),
        )
    });
    let view = runtime.with_authority_for_foundation(|authority| authority.chain_view().clone());
    let candidate = runtime.direct_candidate_for_foundation(
        Arc::clone(&transaction),
        direct_verified_facts_for_view(
            &transaction,
            view,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1_000),
        ),
    );
    let (probe, read_cut_live, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::DirectRejectionReadCutBeforeActivation,
            Some(probe),
        );
    });
    let commit_runtime = runtime.clone();
    let (terminal_tx, terminal_rx) = std::sync::mpsc::sync_channel(1);
    let terminal = std::thread::spawn(move || {
        terminal_tx
            .send(commit_runtime.settle_verified_direct_admission(candidate))
            .expect("the duplicate terminal observer remains alive");
    });
    read_cut_live
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("the duplicate holds its exact owner read cut before activation");
    assert!(
        !runtime.with_authority_read_for_foundation(|authority| {
            authority.owner_shard_write_available_for_foundation(&hash)
        }),
        "the duplicate effect cannot race a replacement of its observed Accepted owner"
    );
    release
        .send(())
        .expect("release the duplicate effect activation");
    let AuthorityDirectAdmissionExecution::Local(execution) = terminal_rx
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("the duplicate terminal returns")
        .expect("the duplicate is a committed Direct outcome")
    else {
        panic!("the Local source retains mutation semantics")
    };
    terminal
        .join()
        .expect("the duplicate terminal does not panic");
    assert!(matches!(
        execution.into_parts().0,
        AuthorityLocalAdmissionOutcome::Duplicate(key) if key == hash
    ));
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::DirectRejectionReadCutBeforeActivation,
            None,
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_production_direct_duplicate_rolls_back_when_its_owner_changes_before_the_read_cut() {
    const TERMINAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let transaction = Arc::new(tx(869));
    let hash = runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(
            authority,
            transaction.as_ref().clone(),
            869,
            AcceptedStatus::Pending,
            Vec::new(),
        )
    });
    let view = runtime.with_authority_for_foundation(|authority| authority.chain_view().clone());
    let candidate = runtime.direct_candidate_for_foundation(
        Arc::clone(&transaction),
        direct_verified_facts_for_view(
            &transaction,
            view,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1_000),
        ),
    );
    let (probe, effect_staged, release) = ConcurrentRemovalProbe::new();
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
            .send(commit_runtime.settle_verified_direct_admission(candidate))
            .expect("the duplicate terminal observer remains alive");
    });
    effect_staged
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("the duplicate effect stages before the final owner read cut");
    assert!(
        runtime
            .remove_local_transaction(&hash.0)
            .expect("the interposed owner removal is an ordinary shared commit")
    );
    release
        .send(())
        .expect("release the now-stale duplicate terminal");
    let AuthorityDirectAdmissionExecution::Local(execution) = terminal_rx
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("the stale duplicate terminal returns")
        .expect("stale Direct work is a typed retry outcome")
    else {
        panic!("the Local source retains mutation semantics")
    };
    terminal
        .join()
        .expect("the duplicate terminal does not panic");
    assert!(matches!(
        execution.into_parts().0,
        AuthorityLocalAdmissionOutcome::Retry(_)
    ));
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::DirectRejectionEffectStagedBeforeReadCut,
            None,
        );
        assert!(authority.entry(&hash).is_none());
        assert!(authority.primary_projection_consistent());
    });
    assert!(runtime.with_authority_for_foundation(|authority| {
        authority
            .effect_trace_for_reference()
            .iter()
            .flat_map(|batch| &batch.effects)
            .all(|effect| {
                !matches!(
                    effect,
                    crate::authority::effect::CommittedEffect::Accepted(
                        crate::authority::effect::CommittedAcceptance::Duplicate { tx_hash, .. }
                    ) if tx_hash == &hash
                )
            })
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_production_direct_existing_owner_commutes_with_same_version_source_promotion() {
    const TERMINAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let raw = tx(870);
    let remote = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"remote-source-race").pack()])
        .build();
    let local = Arc::new(
        raw.as_advanced_builder()
            .set_witnesses(vec![Bytes::from_static(b"local-source-race").pack()])
            .build(),
    );
    let peer = PeerIndex::from(870usize);
    let hash = runtime.with_authority_for_foundation(|authority| {
        let admission = ValidatedAdmission::remote(remote.clone(), peer)
            .expect("the Remote fixture has bounded ingress evidence");
        let hash = admission.identity.raw.clone();
        apply_plan(
            authority
                .plan_admission(admission)
                .expect("the Remote fixture enters PreAccepted ownership"),
        );
        hash
    });
    let old_version =
        runtime.with_authority_for_foundation(|authority| owner_version(authority, &hash));
    let view = runtime.with_authority_for_foundation(|authority| authority.chain_view().clone());
    let candidate = runtime.direct_candidate_for_foundation(
        Arc::clone(&local),
        direct_verified_facts_for_view(
            &local,
            view,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(2_000),
        ),
    );
    let (probe, prepared, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::DirectMembershipPreparedBeforeFinalCut,
            Some(probe),
        );
    });
    let commit_runtime = runtime.clone();
    let (terminal_tx, terminal_rx) = std::sync::mpsc::sync_channel(1);
    let terminal = std::thread::spawn(move || {
        terminal_tx
            .send(commit_runtime.settle_verified_direct_admission(candidate))
            .expect("the Direct terminal observer remains alive");
    });
    prepared
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("the Direct candidate is prepared before its final cut");

    let attempt = proposal(
        BoundedTransaction::try_new(remote).expect("the Proposal fixture is bounded"),
        snapshot.consensus(),
    );
    let batch = RetainedAdmissionBatch::new(attempt, std::collections::VecDeque::new())
        .expect("one Proposal attempt is a homogeneous batch");
    let (consumed, remaining, post_commit_fault) = runtime
        .commit_retained_ingress_batch(batch)
        .unwrap_or_else(|failure| {
            drop(failure);
            panic!("same-witness source promotion commits through the shared ingress route")
        });
    assert_eq!(consumed, 1);
    assert!(remaining.is_empty());
    assert_eq!(post_commit_fault, None);
    assert_eq!(
        runtime.with_authority_read_for_foundation(|authority| owner_version(authority, &hash)),
        old_version,
        "the policy-only source promotion deliberately preserves EntryVersion"
    );

    release.send(()).expect("release the Direct final cut");
    let AuthorityDirectAdmissionExecution::Local(execution) = terminal_rx
        .recv_timeout(TERMINAL_TIMEOUT)
        .expect("the Direct terminal returns")
        .expect("the compatible source promotion and Direct acceptance commute")
    else {
        panic!("the Local source retains mutation semantics")
    };
    terminal.join().expect("the Direct terminal does not panic");
    let (outcome, _) = execution.into_parts();
    assert!(
        matches!(outcome, AuthorityLocalAdmissionOutcome::Accepted(_)),
        "same-version source promotion has the same final Direct owner: {outcome:?}"
    );
    runtime.with_authority_for_foundation(|authority| {
        authority.entries_for_reference().set_shared_ingress_probe(
            SharedIngressProbePhase::DirectMembershipPreparedBeforeFinalCut,
            None,
        );
        assert!(matches!(
            authority.entry(&hash),
            Some(OwnedTx::Accepted(entry))
                if entry.provenance == AcceptedProvenance::Peer { ingress: peer }
                    && entry.record.tx.witness_hash() == local.witness_hash()
        ));
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_production_direct_capacity_eviction_commits_while_an_outer_reader_is_held() {
    let snapshot = genesis_snapshot();
    let resources = ResourceLimits::new(
        ResourceVector::new(8, 64 * 1024, 64, 8),
        ResourceVector::new(4, 32 * 1024, 32, 4),
        ResourceVector::new(2, 16 * 1024, 16, 2),
        AcceptedResources::new(1, 64 * 1024, 64 * 1024, 64),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .and_then(|limits| {
        limits.with_replacement_history_limit(ResourceVector::new(4, 32 * 1024, 32, 0))
    })
    .expect("the one-entry Accepted fixture is valid");
    let runtime = AuthorityRuntime::new_with_resource_limits_for_foundation(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
        resources,
    )
    .expect("the capacity-bound runtime fixture is valid");
    let incumbent_tx = tx(866);
    let incumbent = runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction_with_payload(
            authority,
            incumbent_tx.clone(),
            866,
            AcceptedStatus::Pending,
            resolved_payload_with_facts(
                &incumbent_tx,
                Vec::new(),
                Vec::new(),
                Capacity::shannons(1),
            ),
        )
    });
    let challenger = Arc::new(tx(867));
    let challenger_hash = RawTxHash(challenger.hash());
    let view = runtime.with_authority_for_foundation(|authority| authority.chain_view().clone());
    let candidate = runtime.direct_candidate_for_foundation(
        Arc::clone(&challenger),
        direct_verified_facts_for_view(
            &challenger,
            view,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(10_000),
        ),
    );
    let AuthorityDirectAdmissionExecution::Local(execution) = while_outer_reader_is_held(
        &runtime,
        std::time::Duration::from_secs(2),
        move |runtime| runtime.settle_verified_direct_admission(candidate),
    )
    .expect("capacity eviction commits through the exact shared policy frontier") else {
        panic!("the Local source retains mutation semantics")
    };
    assert!(matches!(
        execution.into_parts().0,
        AuthorityLocalAdmissionOutcome::Accepted(_)
    ));
    runtime.with_authority_for_foundation(|authority| {
        assert!(authority.entry(&incumbent).is_none());
        assert!(matches!(
            authority.entry(&challenger_hash),
            Some(OwnedTx::Accepted(_))
        ));
        assert!(authority.primary_projection_consistent());
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn uak_direct_cache_update_is_released_only_after_local_acceptance() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let parent = output_tx(821, 1_000, Bytes::new());
    runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(
            authority,
            parent.clone(),
            105,
            AcceptedStatus::Pending,
            Vec::new(),
        );
    });
    let transaction = spending_tx(0, [OutPoint::new(parent.hash(), 0)], 900);
    let rules = ScriptVerificationRules::from_env(
        snapshot.consensus(),
        &TxVerifyEnv::new_submit(snapshot.tip_header()),
    );
    let cache_key = TxVerificationCacheKey::from_transaction(&transaction, rules);

    let test_accept = verified_test_accept_candidate(&runtime, &transaction).await;
    let before = runtime.normalized_snapshot_for_foundation();
    let AuthorityDirectAdmissionExecution::TestAccept(AuthorityTestAcceptOutcome::Accepted(
        test_completed,
    )) = runtime
        .settle_verified_direct_admission(test_accept)
        .expect("TestAccept consumes verification evidence without publication")
    else {
        panic!("the valid direct fixture must be accepted read-only")
    };
    assert_eq!(runtime.normalized_snapshot_for_foundation(), before);

    let local = verified_local_candidate(&runtime, &transaction).await;
    let AuthorityDirectAdmissionExecution::Local(local) = runtime
        .settle_verified_direct_admission(local)
        .expect("Local accepts and unlocks the post-commit cache consequence")
    else {
        panic!("the Local source must preserve Local settlement semantics")
    };
    let (outcome, cache_update) = local.into_parts();
    let AuthorityLocalAdmissionOutcome::Accepted(local_completed) = outcome else {
        panic!("the valid direct fixture must commit for Local")
    };
    let update = cache_update.expect("Accepted Local releases the verifier-produced proof");
    let proof = update.into_proof();
    assert_eq!(proof.key(), cache_key);
    assert_eq!(proof.cycles(), test_completed.cycles);
    assert_eq!(proof.cycles(), local_completed.cycles);

    let duplicate = verified_local_candidate(&runtime, &transaction).await;
    let AuthorityDirectAdmissionExecution::Local(duplicate) = runtime
        .settle_verified_direct_admission(duplicate)
        .expect("an Accepted duplicate is a committed acknowledgement only")
    else {
        panic!("the Local source must preserve Local settlement semantics")
    };
    let (outcome, cache_update) = duplicate.into_parts();
    assert!(matches!(
        outcome,
        AuthorityLocalAdmissionOutcome::Duplicate(_)
    ));
    assert!(cache_update.is_none());
}

#[test]
fn uak_direct_owner_free_rejection_terminals_commit_while_an_outer_reader_is_held() {
    const TERMINAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let stable_tx = TransactionBuilder::default().version(1u32).build();
    let stable_execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture has one free transient compute slot");
    let AuthorityDirectResolutionOutcome::Rejected(stable) = runtime
        .resolve_local_transaction(&direct_input(&stable_tx), stable_execution)
        .expect("the stable ingress rejection is typed")
    else {
        panic!("the non-zero transaction version must reject before resolution")
    };

    let validation_tx = Arc::new(
        TransactionBuilder::default()
            .version(6_307u32)
            .input(ckb_types::packed::CellInput::new(
                OutPoint::new(Byte32::new([88; 32]), 0),
                0,
            ))
            .build(),
    );
    let chain_view =
        runtime.with_authority_read_for_foundation(|authority| authority.chain_view().clone());
    let validation = runtime.direct_candidate_for_foundation(
        Arc::clone(&validation_tx),
        direct_verified_facts_for_view(
            &validation_tx,
            chain_view,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1),
        ),
    );

    let (stable_result, validation_result) =
        while_outer_reader_is_held(&runtime, TERMINAL_TIMEOUT, move |runtime| {
            (
                runtime.settle_direct_transaction_rejection(stable),
                runtime.settle_verified_direct_admission(validation),
            )
        });
    assert!(matches!(
        stable_result,
        Ok(AuthorityDirectRejectionExecution::Local(_))
    ));
    let AuthorityDirectAdmissionExecution::Local(validation_result) =
        validation_result.expect("the final validation rejection commits")
    else {
        panic!("the source-sealed validation result remains Local")
    };
    assert!(matches!(
        validation_result.into_parts().0,
        AuthorityLocalAdmissionOutcome::Rejected(DirectAdmissionRejectionKind::Validation(_))
    ));
    assert!(
        runtime
            .pending_recent_reject(&stable_tx.hash())
            .expect("the stable recent-reject projection remains readable")
            .is_some()
    );
    assert!(
        runtime
            .pending_recent_reject(&validation_tx.hash())
            .expect("the validation recent-reject projection remains readable")
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn uak_test_accept_and_local_share_exact_rbf_rejection_policy() {
    let snapshot = genesis_snapshot();
    let mut config = runtime_config();
    config.min_rbf_rate = FeeRate::from_u64(1);
    let runtime = AuthorityRuntime::new(
        &config,
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let parent = output_tx(826, 1_000, Bytes::new());
    let input = OutPoint::new(parent.hash(), 0);
    let existing = spending_tx(827, [input.clone()], 900);
    let existing_hash = crate::authority::state::RawTxHash(existing.hash());
    runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(authority, parent, 101, AcceptedStatus::Pending, Vec::new());
        accept_remote_transaction_with_payload(
            authority,
            existing.clone(),
            102,
            AcceptedStatus::Pending,
            resolved_payload_with_facts(&existing, Vec::new(), Vec::new(), Capacity::shannons(100)),
        );
    });
    let replacement = spending_tx(0, [input], 950);

    let test_accept = verified_test_accept_candidate(&runtime, &replacement).await;
    let before = runtime.normalized_snapshot_for_foundation();
    let AuthorityDirectAdmissionExecution::TestAccept(
        AuthorityTestAcceptOutcome::RejectedMembership(test_reason),
    ) = runtime
        .settle_verified_direct_admission(test_accept)
        .expect("RBF policy rejection is a read-only TestAccept outcome")
    else {
        panic!("the under-fee replacement must be rejected by membership policy")
    };
    assert!(
        matches!(
            test_reason,
            crate::authority::plan::MembershipReject::InsufficientReplacementFee { .. }
        ),
        "unexpected RBF rejection: {test_reason:?}"
    );
    assert_eq!(runtime.normalized_snapshot_for_foundation(), before);

    let local = verified_local_candidate(&runtime, &replacement).await;
    let AuthorityDirectAdmissionExecution::Local(local) = runtime
        .settle_verified_direct_admission(local)
        .expect("Local commits the same RBF rejection and its effect atomically")
    else {
        panic!("the Local source must preserve Local settlement semantics")
    };
    let (
        AuthorityLocalAdmissionOutcome::Rejected(DirectAdmissionRejectionKind::Membership(
            local_reason,
        )),
        cache_update,
    ) = local.into_parts()
    else {
        panic!("the under-fee replacement must remain a membership rejection")
    };
    assert!(cache_update.is_none());
    assert!(matches!(
        local_reason,
        crate::authority::plan::MembershipReject::InsufficientReplacementFee { .. }
    ));
    runtime.with_authority_for_foundation(|authority| {
        assert!(matches!(
            authority.entry(&existing_hash),
            Some(OwnedTx::Accepted(_))
        ));
        assert!(
            authority
                .entry(&crate::authority::state::RawTxHash(replacement.hash()))
                .is_none()
        );
    });
    assert!(
        runtime
            .pending_recent_reject(&replacement.hash())
            .expect("the RBF rejection projects to the recent-reject surface")
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn uak_local_atomically_replaces_same_raw_active_remote_owner() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let parent = output_tx(828, 1_000, Bytes::new());
    runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(
            authority,
            parent.clone(),
            103,
            AcceptedStatus::Pending,
            Vec::new(),
        );
    });
    let transaction = spending_tx(0, [OutPoint::new(parent.hash(), 0)], 900);
    let local = verified_local_candidate(&runtime, &transaction).await;
    runtime
        .submit_remote_ingress(transaction.clone(), 0, PeerIndex::from(104))
        .expect("the Remote owner is retained before Local wins");
    let active = match runtime
        .try_checkout_for_foundation(WorkPermit::ResolveOnly)
        .expect("the retained scheduler remains healthy")
    {
        ControlFlow::Continue(Some(active)) => active,
        ControlFlow::Continue(None) => panic!("the Remote owner enters active resolve work"),
        ControlFlow::Break(_) => panic!("the fixture has sufficient effect capacity"),
    };

    let AuthorityDirectAdmissionExecution::Local(local) = runtime
        .settle_verified_direct_admission(local)
        .expect("Local replaces the exact same-raw active owner in one Apply")
    else {
        panic!("the Local source must preserve Local settlement semantics")
    };
    assert!(matches!(
        local.into_parts().0,
        AuthorityLocalAdmissionOutcome::Accepted(_)
    ));
    runtime.with_authority_for_foundation(|authority| {
        assert!(matches!(
            authority.entry(&crate::authority::state::RawTxHash(transaction.hash())),
            Some(OwnedTx::Accepted(_))
        ));
        assert_eq!(authority.resources().preaccepted().active_work, 0);
    });
    let stale = runtime.settle_compute(
        active.retry_for_foundation(),
        crate::authority::runtime::SettlementOrigin::Completion,
    );
    assert!(matches!(
        stale,
        ControlFlow::Break(pending)
            if matches!(
                pending.recovery(),
                crate::authority::plan::ComputeSettlementRecovery::Obsolete(_)
            )
    ));
}
