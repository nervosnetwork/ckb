use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, apply_plan, limits,
    owner_version, resolved_payload_with_facts, runtime_config,
};
use crate::authority::{
    ingress::{BoundedTransaction, DirectCommand, DirectIngressTransaction, direct},
    plan::TxPoolAuthority,
    resolver::{
        DirectResolutionEvaluation, DirectResolutionJob, DirectResolutionPreparation,
        DirectResolutionProbeObservation, DirectVerificationOutcome, ResolutionEvaluation,
        ResolutionExecutionKind, ResolutionJob, ResolutionProbeObservation, VerificationExecution,
        VerificationJob, VerificationTimePolicy, VerificationTimePolicyError,
    },
    runtime::{
        AuthorityDirectAdmissionError, AuthorityDirectAdmissionExecution,
        AuthorityDirectRejectionExecution, AuthorityDirectResolutionOutcome,
        AuthorityDirectVerificationOutcome, AuthorityDirectVerifiedCandidate,
        AuthorityLocalAdmissionOutcome, AuthorityRuntime, AuthorityTestAcceptOutcome,
        DirectAdmissionRejectionKind,
    },
    state::{
        AcceptedStatus, ChainRevision, ChainViewId, DependencyKey, OwnedTx, PayloadPolicy,
        PreAcceptedPhase, QueuedWork, ValidatedAdmission, VerifyCapability, WorkPermit,
    },
    work::{CheckedOutWork, ResolutionEvidence, SettlementNext, SettlementRejection},
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
    core::{BlockExt, Capacity, DepType, FeeRate, TransactionBuilder, cell::ResolvedTransaction},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint, OutPointVec},
    prelude::{Builder, Entity, Pack, Unpack},
};
use ckb_verification::cache::{ScriptVerificationRules, TxVerificationCacheKey, init_cache};
use std::{ops::ControlFlow, sync::Arc};

fn verification_deadline() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(30)
}

fn initial_load_limit() -> ckb_script::InitialProgramLoadLimit {
    ckb_script::InitialProgramLoadLimit::new(u64::MAX)
        .expect("the test initial-load limit is non-zero")
}

fn verification_budget_at(deadline: std::time::Instant) -> crate::util::TxPoolVerificationBudget {
    crate::util::TxPoolVerificationBudget::new(deadline, initial_load_limit())
}

fn verification_budget() -> crate::util::TxPoolVerificationBudget {
    verification_budget_at(verification_deadline())
}

#[test]
fn uak_verification_time_policy_is_fixed_bounded_and_never_peer_extended() {
    let policy = VerificationTimePolicy::from_runtime(250, 10_000, 30_000)
        .expect("the fixture policy is valid");
    let started_at = std::time::Instant::now();
    let hard_deadline = started_at + std::time::Duration::from_secs(30);

    assert_eq!(
        policy.deadline(
            started_at,
            hard_deadline,
            PayloadPolicy::remote_for_foundation(1),
        ),
        started_at + std::time::Duration::from_millis(250),
        "a low peer declaration receives only the fixed minimum budget"
    );
    assert_eq!(
        policy.deadline(
            started_at,
            hard_deadline,
            PayloadPolicy::remote_for_foundation(2_500_001),
        ),
        started_at + std::time::Duration::from_millis(251),
        "the cycle signal rounds up rather than truncating a partial millisecond"
    );
    let slow_signal_policy = VerificationTimePolicy::from_runtime(250, 1, 30_000)
        .expect("the slow-signal fixture policy is valid");
    assert_eq!(
        slow_signal_policy.deadline(
            started_at,
            hard_deadline,
            PayloadPolicy::remote_for_foundation(70_000_000),
        ),
        hard_deadline,
        "an untrusted declaration can never extend the unconditional hard cap"
    );
    assert_eq!(
        policy.deadline(started_at, hard_deadline, PayloadPolicy::Trusted),
        hard_deadline,
        "trusted local/proposal work is constrained only by the node hard cap"
    );
}

#[test]
fn uak_verification_time_policy_rejects_unusable_node_configuration() {
    assert_eq!(
        VerificationTimePolicy::from_runtime(250, 0, 30_000).err(),
        Some(VerificationTimePolicyError::ZeroCycleRate)
    );
    for (minimum, maximum) in [(0, 30_000), (30_001, 30_000)] {
        assert_eq!(
            VerificationTimePolicy::from_runtime(minimum, 10_000, maximum).err(),
            Some(VerificationTimePolicyError::InvalidDurationRange)
        );
    }
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
    let outcome = runtime
        .execute_direct_verification(request.bind_cache(&cache), None)
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
    // The small arguments in this module model fee differences, not raw CKB
    // occupied-capacity units.  Keep that arithmetic while making every
    // output a transaction-verifier-valid cell now that direct-cache tests
    // obtain proofs from the real hot/cold path instead of forging cycles.
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
        .plan_checkout_for_foundation(
            &key,
            owner_version(authority, &key),
            WorkPermit::ResolveOnly,
        )
        .expect("resolve checkout plans")
        .apply();
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
        .plan_checkout_for_foundation(
            &key,
            owner_version(authority, &key),
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("verification checkout plans")
        .apply();
    let CheckedOutWork::Verify(work) = checkout.into_work() else {
        panic!("verify-only checkout must carry verification work")
    };
    VerificationJob::from_checkout(work, snapshot)
        .expect("verification remains on the resolve snapshot")
}

#[test]
fn uak_resolution_job_rejects_a_mixed_snapshot_view() {
    let snapshot = genesis_snapshot();
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let work = checkout_resolve(
        &mut authority,
        TransactionBuilder::default().version(801u32).build(),
        81,
    );
    let failure = ResolutionJob::capture_resolve(&authority, snapshot, work)
        .expect_err("a snapshot from another chain cut cannot enter resolution");
    assert_eq!(failure.kind(), ResolutionExecutionKind::StaleView);
    apply_plan(
        authority
            .apply_settlement(failure.into_settlement())
            .expect("the exact active capability retries under the authority view"),
    );
}

#[test]
fn uak_duplicate_inputs_are_a_malformed_outcome_not_an_authority_fault() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let input = OutPoint::new(Byte32::new([0x80; 32]), 0);
    let transaction = spending_tx(800, [input.clone(), input], 1);
    let work = checkout_resolve(&mut authority, transaction.clone(), 80);
    let settlement = work
        .resolved(ResolutionEvidence::for_foundation(
            Arc::new(ResolvedTransaction::dummy_resolve(transaction)),
            Capacity::zero(),
            1_024,
            crate::authority::state::VerifyCycleClass::Small,
        ))
        .expect("duplicate inputs are a deterministic transaction outcome");
    let SettlementNext::Rejected(SettlementRejection::ChainBound(reason)) = settlement.next else {
        panic!("duplicate inputs must not become retry or structural evidence")
    };
    assert!(matches!(reason.reject(), Reject::Malformed(..)));
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
        .plan_checkout_for_foundation(
            &key,
            owner_version(&authority, &key),
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("the stale queued verification can be checked out without a pool scan")
        .apply();
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

#[test]
fn uak_resolution_reports_the_complete_direct_missing_frontier() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let first = OutPoint::new(Byte32::new([0x81; 32]), 0);
    let second = OutPoint::new(Byte32::new([0x82; 32]), 0);
    let work = checkout_resolve(
        &mut authority,
        spending_tx(802, [first.clone(), second.clone()], 1),
        82,
    );
    let job = ResolutionJob::capture_resolve(&authority, Arc::clone(&snapshot), work)
        .expect("the checked-out view owns this snapshot");
    let ResolutionEvaluation::Enrich(probe) = job
        .evaluate(FeeRate::zero(), u64::MAX)
        .expect("missing cells are a normal resolution outcome")
    else {
        panic!("unknown inputs must request enrichment")
    };
    assert_eq!(
        probe.missing_keys_for_foundation(),
        vec![DependencyKey::Cell(first), DependencyKey::Cell(second)]
    );
    let ResolutionProbeObservation::Missing(probe) = probe
        .prepare_enrichment()
        .expect("the bounded probe reserves outside the authority cut")
        .observe(&authority)
    else {
        panic!("no Accepted producer exists")
    };
    let settlement = probe
        .settle_missing()
        .expect("an unchanged authority cut settles the missing frontier");
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("the missing observation is current"),
    );
}

#[test]
fn uak_resolution_reads_only_the_needed_accepted_parent() {
    let snapshot = genesis_snapshot();
    let parent = output_tx(803, 1_000, Bytes::new());
    let parent_hash = parent.hash();
    let mut alternate_bytes = parent_hash.as_slice().to_vec();
    alternate_bytes[31] ^= 1;
    let alternate_hash =
        Byte32::from_slice(&alternate_bytes).expect("the alternate hash is fixed-size");
    assert_eq!(
        ckb_types::packed::ProposalShortId::from_tx_hash(&parent_hash),
        ckb_types::packed::ProposalShortId::from_tx_hash(&alternate_hash)
    );

    let mut collision_authority = authority_at(&snapshot);
    accept_remote_transaction(
        &mut collision_authority,
        parent.clone(),
        83,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let colliding_child = spending_tx(804, [OutPoint::new(alternate_hash.clone(), 0)], 900);
    let colliding_work = checkout_resolve(&mut collision_authority, colliding_child, 84);
    let colliding_job =
        ResolutionJob::capture_resolve(&collision_authority, Arc::clone(&snapshot), colliding_work)
            .expect("the sparse cut captures no differently hashed producer");
    let ResolutionEvaluation::Enrich(colliding_probe) = colliding_job
        .evaluate(FeeRate::zero(), u64::MAX)
        .expect("the colliding raw-hash miss is a normal resolution outcome")
    else {
        panic!("a matching proposal short ID must not resolve the alternate raw producer")
    };
    assert_eq!(
        colliding_probe.missing_keys_for_foundation(),
        vec![DependencyKey::Cell(OutPoint::new(alternate_hash, 0))]
    );
    assert!(matches!(
        colliding_probe
            .prepare_enrichment()
            .expect("the one-key collision probe reserves")
            .observe(&collision_authority),
        ResolutionProbeObservation::Missing(_)
    ));

    let mut authority = authority_at(&snapshot);
    accept_remote_transaction(
        &mut authority,
        parent,
        83,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let child = spending_tx(805, [OutPoint::new(parent_hash, 0)], 900);
    let child_hash = child.hash();
    let work = checkout_resolve(&mut authority, child, 84);
    let job = ResolutionJob::capture_resolve(&authority, Arc::clone(&snapshot), work)
        .expect("the sparse overlay captures the Accepted parent");
    let ResolutionEvaluation::Settle(settlement) = job
        .evaluate(FeeRate::zero(), u64::MAX)
        .expect("the Accepted output resolves normally")
    else {
        panic!("resolve-only work must queue verification")
    };
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("the parent proof is current"),
    );
    assert!(matches!(
        authority.entry(&crate::authority::state::RawTxHash(child_hash)),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
    ));
}

#[test]
fn uak_resolution_enrichment_is_bounded_and_stale_safe() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let parent = output_tx(805, 1_000, Bytes::new());
    let parent_out = OutPoint::new(parent.hash(), 0);
    let child = spending_tx(806, [parent_out], 900);
    let work = checkout_resolve(&mut authority, child, 86);
    let job = ResolutionJob::capture_resolve(&authority, Arc::clone(&snapshot), work)
        .expect("the initial sparse cut is valid");
    let ResolutionEvaluation::Enrich(probe) = job
        .evaluate(FeeRate::zero(), u64::MAX)
        .expect("the absent parent is a normal miss")
    else {
        panic!("the first cut has no parent")
    };

    accept_remote_transaction(
        &mut authority,
        parent,
        85,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let ResolutionProbeObservation::Retry(job) = probe
        .prepare_enrichment()
        .expect("the bounded probe reserves outside the authority cut")
        .observe(&authority)
    else {
        panic!("new evidence requires exactly one retry")
    };
    let ResolutionEvaluation::Settle(settlement) = job
        .evaluate(FeeRate::zero(), u64::MAX)
        .expect("the enriched job resolves")
    else {
        panic!("the one missing producer was supplied")
    };
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("availability after checkout does not invalidate positive evidence"),
    );
}

#[test]
fn uak_resolution_discovers_every_available_dep_group_member_miss() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let first = OutPoint::new(Byte32::new([0x91; 32]), 0);
    let second = OutPoint::new(Byte32::new([0x92; 32]), 0);
    let group = OutPointVec::new_builder()
        .set(vec![first.clone(), second.clone()])
        .build();
    let group_parent = output_tx(807, 1_000, group.as_bytes());
    accept_remote_transaction(
        &mut authority,
        group_parent.clone(),
        87,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let group_out = OutPoint::new(group_parent.hash(), 0);
    let child = TransactionBuilder::default()
        .version(808u32)
        .cell_dep(
            CellDep::new_builder()
                .out_point(group_out)
                .dep_type(DepType::DepGroup)
                .build(),
        )
        .build();
    let work = checkout_resolve(&mut authority, child, 88);
    let job = ResolutionJob::capture_resolve(&authority, Arc::clone(&snapshot), work)
        .expect("the direct dep-group producer is captured");
    let ResolutionEvaluation::Enrich(probe) = job
        .evaluate(FeeRate::zero(), u64::MAX)
        .expect("expanded misses are a normal outcome")
    else {
        panic!("missing dep-group members require enrichment")
    };
    assert_eq!(
        probe.missing_keys_for_foundation(),
        vec![DependencyKey::Cell(first), DependencyKey::Cell(second)]
    );
}

#[test]
fn uak_permissive_rbf_resolution_never_fabricates_a_chain_cell() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let parent = output_tx(809, 1_000, Bytes::new());
    let parent_out = OutPoint::new(parent.hash(), 0);
    accept_remote_transaction(
        &mut authority,
        parent,
        89,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let existing = spending_tx(810, [parent_out.clone()], 900);
    accept_remote_transaction(
        &mut authority,
        existing,
        90,
        AcceptedStatus::Pending,
        Vec::new(),
    );

    // Keep the unknown input first: the consensus resolver stops there before
    // observing the later pool conflict. The tx-pool permissive retry may
    // ignore that Accepted spend, but it must still consult the chain snapshot
    // for the unknown cell.
    let unknown = OutPoint::new(Byte32::new([0xa1; 32]), 0);
    let replacement = spending_tx(811, [unknown.clone(), parent_out], 800);
    let work = checkout_resolve(&mut authority, replacement, 91);
    let job = ResolutionJob::capture_resolve(&authority, Arc::clone(&snapshot), work)
        .expect("the conflict spend is captured");
    let ResolutionEvaluation::Enrich(probe) = job
        .evaluate(FeeRate::zero(), u64::MAX)
        .expect("permissive mode still observes the chain snapshot")
    else {
        panic!("an unknown chain cell must not become resolved RBF evidence")
    };
    assert_eq!(
        probe.missing_keys_for_foundation(),
        vec![DependencyKey::Cell(unknown)]
    );
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

    let execution = request.bind_cache(&cache).execute(None).await;
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
async fn uak_expired_verification_deadline_is_transient_and_never_cached() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let transaction = TransactionBuilder::default().version(8_120u32).build();
    let request =
        checkout_verification_job(&mut authority, Arc::clone(&snapshot), transaction, 9_120)
            .prepare(verification_budget_at(
                std::time::Instant::now() - std::time::Duration::from_millis(1),
            ));
    let cache = init_cache();
    let (_command_tx, mut command_rx) = tokio::sync::watch::channel(ChunkCommand::Resume);

    let VerificationExecution {
        settlement,
        cache_update,
    } = request
        .bind_cache(&cache)
        .execute(Some(&mut command_rx))
        .await;
    assert!(cache_update.is_none(), "a local timeout is never VM proof");
    assert!(
        matches!(
            &settlement.next,
            SettlementNext::VerificationRejected { rejection, .. }
                if matches!(rejection.reject(), Reject::ExcessiveVerifyTime)
                    && !rejection.should_record()
                    && rejection.publish_negative_relay_terminal()
        ),
        "unexpected expired-deadline settlement: {:?}",
        settlement.next
    );
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("the exact timed-out verification capability settles once"),
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
                verification_deadline(),
                ckb_script::InitialProgramLoadLimit::new(1)
                    .expect("the rejecting fixture limit is non-zero"),
            ));
    let cache = init_cache();
    let (_command_tx, mut command_rx) = tokio::sync::watch::channel(ChunkCommand::Resume);

    let VerificationExecution {
        settlement,
        cache_update,
    } = request
        .bind_cache(&cache)
        .execute(Some(&mut command_rx))
        .await;
    assert!(
        cache_update.is_none(),
        "a local load refusal is never VM proof"
    );
    assert!(
        matches!(
            &settlement.next,
            SettlementNext::VerificationRejected { rejection, .. }
                if matches!(rejection.reject(), Reject::ExcessiveVerifyTime)
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
    let cached = checkout_verification_job(&mut authority, Arc::clone(&snapshot), cached_tx, 93)
        .prepare(verification_budget())
        .bind_cache(&cache)
        .execute(None)
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

    let execution = request.bind_cache(&cache).execute(None).await;
    assert!(execution.cache_update.is_some());
    apply_plan(
        authority
            .apply_settlement(execution.settlement)
            .expect("the cache-miss verification capability settles"),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn uak_direct_resolution_reads_accepted_without_acquiring_an_owner() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let parent = output_tx(813, 1_000, Bytes::new());
    accept_remote_transaction(
        &mut authority,
        parent.clone(),
        93,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let child = Arc::new(spending_tx(814, [OutPoint::new(parent.hash(), 0)], 900));
    let child_key = crate::authority::state::RawTxHash(child.hash());
    let before = authority.normalized_snapshot();
    let job = DirectResolutionJob::capture_for_foundation(
        &authority,
        Arc::clone(&snapshot),
        Arc::clone(&child),
        1 << 20,
        1_000,
    )
    .expect("the direct cut captures the accepted parent without retaining the child");
    let DirectResolutionEvaluation::Verify(request) = job
        .evaluate(FeeRate::zero(), verification_budget())
        .expect("the accepted output resolves under the paired cut")
    else {
        panic!("the direct child must continue to verification")
    };
    let expected_rules = ScriptVerificationRules::from_env(
        snapshot.consensus(),
        &TxVerifyEnv::new_submit(snapshot.tip_header()),
    );
    let witness_hash: [u8; 32] = child.witness_hash().unpack();
    let expected_key = TxVerificationCacheKey::from_transaction(&child, expected_rules);
    assert_eq!(expected_key.witness_hash(), &witness_hash);
    let cache = init_cache();

    let DirectVerificationOutcome::Candidate(candidate) = request
        .bind_cache(&cache)
        .execute(None)
        .await
        .expect("the snapshot-bound direct request verifies")
    else {
        panic!("the cached direct verification must produce admission work")
    };
    let (command, work, cache_update) = candidate.into_parts();
    assert_eq!(command, DirectCommand::TestAccept);
    assert_eq!(
        cache_update
            .expect("owner-free verification still derives a real proof")
            .into_proof()
            .key(),
        expected_key
    );
    assert_eq!(work.payload().identity().raw, child_key);
    assert!(authority.entry(&child_key).is_none());
    assert_eq!(authority.normalized_snapshot(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn uak_direct_expired_deadline_returns_the_same_transient_local_rejection() {
    let snapshot = genesis_snapshot();
    let authority = authority_at(&snapshot);
    let before = authority.normalized_snapshot();
    let transaction = Arc::new(TransactionBuilder::default().version(8_121u32).build());
    let job = DirectResolutionJob::capture_for_foundation(
        &authority,
        Arc::clone(&snapshot),
        transaction,
        1 << 20,
        1_000,
    )
    .expect("the bounded direct fixture captures one coherent read cut");
    let DirectResolutionEvaluation::Verify(request) = job
        .evaluate(
            FeeRate::zero(),
            verification_budget_at(std::time::Instant::now() - std::time::Duration::from_millis(1)),
        )
        .expect("the direct fixture resolves before script verification")
    else {
        panic!("the direct fixture must reach the deadline-bound verifier")
    };
    let cache = init_cache();
    let (_command_tx, mut command_rx) = tokio::sync::watch::channel(ChunkCommand::Resume);

    let DirectVerificationOutcome::Rejected(rejection) = request
        .bind_cache(&cache)
        .execute(Some(&mut command_rx))
        .await
        .expect("deadline expiry is a typed local outcome")
    else {
        panic!("an already-expired direct request cannot become a candidate")
    };
    assert!(matches!(
        rejection.reason().reject(),
        Reject::ExcessiveVerifyTime
    ));
    assert!(!rejection.reason().should_record());
    assert!(rejection.reason().publish_negative_relay_terminal());
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_direct_resolution_terminalizes_an_unchanged_missing_frontier() {
    let snapshot = genesis_snapshot();
    let authority = authority_at(&snapshot);
    let missing = OutPoint::new(Byte32::new([0xb1; 32]), 0);
    let tx = Arc::new(spending_tx(815, [missing.clone()], 1));
    let before = authority.normalized_snapshot();
    let job = DirectResolutionJob::capture_for_foundation(
        &authority,
        Arc::clone(&snapshot),
        tx,
        1 << 20,
        1_000,
    )
    .expect("the direct request captures a coherent empty overlay");
    let DirectResolutionEvaluation::Enrich(probe) = job
        .evaluate(FeeRate::zero(), verification_budget())
        .expect("missing evidence is a transaction outcome")
    else {
        panic!("the first pass must expose the missing frontier")
    };
    assert_eq!(
        probe.missing_keys_for_foundation(),
        vec![DependencyKey::Cell(missing.clone())]
    );
    let DirectResolutionProbeObservation::Rejected(rejection) = probe
        .prepare_enrichment()
        .expect("bounded enrichment reserves outside the authority cut")
        .observe(&authority)
        .finish()
        .expect("the authority view remains current")
    else {
        panic!("an unchanged missing direct input must reject, not become resident")
    };
    assert!(matches!(
        rejection.reason().reject(),
        crate::error::Reject::Resolve(ckb_types::core::error::OutPointError::Unknown(out_point))
            if out_point == &missing
    ));
    assert_eq!(authority.normalized_snapshot(), before);
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

#[tokio::test(flavor = "multi_thread")]
async fn uak_runtime_direct_path_is_owner_free_until_membership_plan() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let parent = output_tx(819, 1_000, Bytes::new());
    runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(
            authority,
            parent.clone(),
            96,
            AcceptedStatus::Pending,
            Vec::new(),
        );
    });
    let transaction = spending_tx(0, [OutPoint::new(parent.hash(), 0)], 900);
    let before = runtime.normalized_snapshot_for_foundation();
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture has one free transient compute slot");
    let input = direct_input(&transaction);
    let outcome = runtime
        .resolve_test_accept_transaction(&input, execution)
        .expect("non-contextual validation and direct resolution succeed");
    let request = match outcome {
        AuthorityDirectResolutionOutcome::Verification(request) => request,
        AuthorityDirectResolutionOutcome::Rejected(rejection) => {
            panic!(
                "the valid direct transaction must continue to verification: {:?}",
                rejection.reason().reject()
            )
        }
    };
    let cache = init_cache();
    let AuthorityDirectVerificationOutcome::Candidate(candidate) = runtime
        .execute_direct_verification(request.bind_cache(&cache), None)
        .await
        .expect("direct verification produces immutable admission work")
    else {
        panic!("the valid direct transaction must remain a candidate")
    };
    let AuthorityDirectAdmissionExecution::TestAccept(outcome) = runtime
        .settle_verified_direct_admission(candidate)
        .expect("the direct candidate validates against a coherent final cut")
    else {
        panic!("the TestAccept source must preserve read-only settlement semantics")
    };
    assert!(matches!(outcome, AuthorityTestAcceptOutcome::Accepted(_)));
    assert_eq!(runtime.normalized_snapshot_for_foundation(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn uak_test_accept_is_read_only_and_local_applies_the_same_policy() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let parent = output_tx(820, 1_000, Bytes::new());
    runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(
            authority,
            parent.clone(),
            97,
            AcceptedStatus::Pending,
            Vec::new(),
        );
    });
    let transaction = spending_tx(0, [OutPoint::new(parent.hash(), 0)], 900);

    let test_accept = verified_test_accept_candidate(&runtime, &transaction).await;
    let before = runtime.normalized_snapshot_for_foundation();
    let AuthorityDirectAdmissionExecution::TestAccept(AuthorityTestAcceptOutcome::Accepted(
        completed,
    )) = runtime
        .settle_verified_direct_admission(test_accept)
        .expect("read-only membership policy accepts the candidate")
    else {
        panic!("the valid independent candidate must be accepted")
    };
    assert!(
        completed.cycles > 0,
        "the direct fixture must carry cycles produced by the real VM"
    );
    assert_eq!(runtime.normalized_snapshot_for_foundation(), before);

    let local = verified_local_candidate(&runtime, &transaction).await;
    let AuthorityDirectAdmissionExecution::Local(local) = runtime
        .settle_verified_direct_admission(local)
        .expect("Local compiles the same policy result into one Apply")
    else {
        panic!("the Local source must preserve Local settlement semantics")
    };
    let (AuthorityLocalAdmissionOutcome::Accepted(local_completed), cache_update) =
        local.into_parts()
    else {
        panic!("the same candidate must commit for Local")
    };
    let update = cache_update.expect("a Local cache miss releases its executed proof after Apply");
    assert_eq!(update.into_proof().cycles(), local_completed.cycles);
    assert_eq!(local_completed, completed);
    runtime.with_authority_for_foundation(|authority| {
        assert!(matches!(
            authority.entry(&crate::authority::state::RawTxHash(transaction.hash())),
            Some(OwnedTx::Accepted(_))
        ));
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

#[tokio::test(flavor = "multi_thread")]
async fn uak_test_accept_treats_every_owner_phase_as_a_read_only_duplicate() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let parent = output_tx(822, 1_000, Bytes::new());
    runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(
            authority,
            parent.clone(),
            98,
            AcceptedStatus::Pending,
            Vec::new(),
        );
    });
    let transaction = spending_tx(0, [OutPoint::new(parent.hash(), 0)], 900);
    let validated = verified_test_accept_candidate(&runtime, &transaction).await;
    runtime
        .submit_remote_ingress(transaction.clone(), 0, PeerIndex::from(99))
        .expect("the same raw transaction becomes a retained Remote owner");
    let before = runtime.normalized_snapshot_for_foundation();
    assert!(matches!(
        runtime
            .settle_verified_direct_admission(validated)
            .expect("duplicate evaluation is a normal read-only outcome"),
        AuthorityDirectAdmissionExecution::TestAccept(AuthorityTestAcceptOutcome::Duplicate(
            key
        )) if key == crate::authority::state::RawTxHash(transaction.hash())
    ));
    assert_eq!(runtime.normalized_snapshot_for_foundation(), before);
}

#[test]
fn uak_direct_missing_rejection_stales_when_the_parent_becomes_available() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let parent = output_tx(824, 1_000, Bytes::new());
    let transaction = spending_tx(0, [OutPoint::new(parent.hash(), 0)], 900);
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture has one free transient compute slot");
    let input = direct_input(&transaction);
    let AuthorityDirectResolutionOutcome::Rejected(rejection) = runtime
        .resolve_local_transaction(&input, execution)
        .expect("the unchanged missing frontier is a transaction outcome")
    else {
        panic!("the missing parent must not reach verification")
    };

    runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(authority, parent, 100, AcceptedStatus::Pending, Vec::new());
    });
    let before = runtime.normalized_snapshot_for_foundation();
    let result = runtime.settle_direct_transaction_rejection(rejection);
    assert!(
        matches!(result, Err(AuthorityDirectAdmissionError::Stale)),
        "new parent availability must stale the missing proof: {result:?}"
    );
    assert_eq!(runtime.normalized_snapshot_for_foundation(), before);
}

#[test]
fn uak_direct_missing_rejection_ignores_an_unrelated_accepted_commit() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let missing_parent = output_tx(825, 1_000, Bytes::new());
    let transaction = spending_tx(0, [OutPoint::new(missing_parent.hash(), 0)], 900);
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture has one free transient compute slot");
    let input = direct_input(&transaction);
    let AuthorityDirectResolutionOutcome::Rejected(rejection) = runtime
        .resolve_local_transaction(&input, execution)
        .expect("the unchanged missing frontier is a transaction outcome")
    else {
        panic!("the missing parent must not reach verification")
    };

    runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(
            authority,
            output_tx(826, 1_000, Bytes::new()),
            101,
            AcceptedStatus::Pending,
            Vec::new(),
        );
    });
    assert!(matches!(
        runtime
            .settle_direct_transaction_rejection(rejection)
            .expect("an unrelated Accepted owner preserves the exact negative read set"),
        AuthorityDirectRejectionExecution::Local(_)
    ));
}

#[test]
fn uak_direct_missing_rejection_stales_when_a_queried_input_gains_a_spender() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let contested_input = OutPoint::new(Byte32::new([83; 32]), 0);
    let permanently_missing = OutPoint::new(Byte32::new([84; 32]), 0);
    let transaction = spending_tx(0, [contested_input.clone(), permanently_missing], 900);
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture has one free transient compute slot");
    let input = direct_input(&transaction);
    let AuthorityDirectResolutionOutcome::Rejected(rejection) = runtime
        .resolve_local_transaction(&input, execution)
        .expect("the unchanged missing frontier is a transaction outcome")
    else {
        panic!("the missing inputs must not reach verification")
    };

    runtime.with_authority_for_foundation(|authority| {
        let competitor = spending_tx(827, [contested_input.clone()], 800);
        accept_remote_transaction_with_payload(
            authority,
            competitor.clone(),
            102,
            AcceptedStatus::Pending,
            resolved_payload_with_facts(
                &competitor,
                Vec::new(),
                vec![contested_input],
                Capacity::shannons(100),
            ),
        );
    });
    let before = runtime.normalized_snapshot_for_foundation();
    let result = runtime.settle_direct_transaction_rejection(rejection);
    assert!(
        matches!(result, Err(AuthorityDirectAdmissionError::Stale)),
        "a new Accepted spender of a queried input must stale the negative proof: {result:?}"
    );
    assert_eq!(runtime.normalized_snapshot_for_foundation(), before);
}

#[test]
fn uak_stable_direct_rejection_is_read_only_for_test_accept_and_atomic_for_local() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let transaction = TransactionBuilder::default().version(1u32).build();

    let test_execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture has one free transient compute slot");
    let test_input = direct_input(&transaction);
    let AuthorityDirectResolutionOutcome::Rejected(test_rejection) = runtime
        .resolve_test_accept_transaction(&test_input, test_execution)
        .expect("non-contextual rejection is a stable transaction outcome")
    else {
        panic!("a non-zero transaction version must reject before resolution")
    };
    let before = runtime.normalized_snapshot_for_foundation();
    let AuthorityDirectRejectionExecution::TestAccept(reason) = runtime
        .settle_direct_transaction_rejection(test_rejection)
        .expect("TestAccept may return stable evidence without mutation")
    else {
        panic!("the TestAccept source must preserve read-only settlement semantics")
    };
    assert!(matches!(
        reason.reject(),
        crate::error::Reject::Verification(_)
    ));
    assert_eq!(runtime.normalized_snapshot_for_foundation(), before);

    let local_execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the settled TestAccept slot is available to Local");
    let local_input = direct_input(&transaction);
    let AuthorityDirectResolutionOutcome::Rejected(local_rejection) = runtime
        .resolve_local_transaction(&local_input, local_execution)
        .expect("the same stable rejection is reproducible")
    else {
        panic!("a non-zero transaction version must reject before resolution")
    };
    assert!(matches!(
        runtime
            .settle_direct_transaction_rejection(local_rejection)
            .expect("Local publishes the rejection in one effect-only Apply"),
        AuthorityDirectRejectionExecution::Local(_)
    ));
    assert!(
        runtime
            .pending_recent_reject(&transaction.hash())
            .expect("the committed rejection has a valid public projection")
            .is_some(),
        "the rejection effect must be visible immediately after Apply"
    );
    runtime.with_authority_for_foundation(|authority| {
        assert!(
            authority
                .entry(&crate::authority::state::RawTxHash(transaction.hash()))
                .is_none()
        );
    });
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
