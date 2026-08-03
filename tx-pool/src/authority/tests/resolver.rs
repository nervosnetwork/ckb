use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, apply_without_work, limits,
    owner_version, resolved_payload_with_facts, runtime_config,
};
use crate::authority::{
    ingress::{DirectCommand, direct},
    plan::TxPoolAuthority,
    resolver::{
        DirectResolutionEvaluation, DirectResolutionJob, DirectResolutionPreparation,
        DirectResolutionProbeObservation, DirectVerificationOutcome, ResolutionEvaluation,
        ResolutionExecutionKind, ResolutionJob, ResolutionProbeObservation, VerificationJob,
    },
    runtime::{
        AuthorityDirectAdmissionError, AuthorityDirectAdmissionExecution,
        AuthorityDirectRejectionExecution, AuthorityDirectResolutionOutcome,
        AuthorityDirectVerificationOutcome, AuthorityDirectVerifiedCandidate,
        AuthorityLocalAdmissionOutcome, AuthorityRuntime, AuthorityTestAcceptOutcome,
        DirectAdmissionRejectionKind,
    },
    state::{
        AcceptedStatus, ChainRevision, ChainViewId, DependencyKey, OwnedTx, PreAcceptedPhase,
        QueuedWork, ValidatedAdmission, VerifyCapability, WorkPermit,
    },
    work::{CheckedOutWork, ResolutionEvidence, SettlementNext, SettlementRejection},
};
use crate::error::Reject;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_network::PeerIndex;
use ckb_script::TxVerifyEnv;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    U256,
    bytes::Bytes,
    core::{Capacity, DepType, FeeRate, TransactionBuilder, cell::ResolvedTransaction},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint, OutPointVec},
    prelude::{Builder, Entity, Pack, Unpack},
};
use ckb_verification::cache::{
    Completed, ScriptVerificationRules, TxVerificationCache, TxVerificationCacheKey, init_cache,
};
use std::{ops::ControlFlow, sync::Arc};

fn completed_cache(
    snapshot: &Snapshot,
    transaction: &ckb_types::core::TransactionView,
) -> TxVerificationCache {
    let rules = ScriptVerificationRules::from_env(
        snapshot.consensus(),
        &TxVerifyEnv::new_submit(snapshot.tip_header()),
    );
    let mut cache = init_cache();
    cache.put(
        TxVerificationCacheKey::from_transaction(transaction, rules),
        Completed {
            cycles: 0,
            fee: Capacity::zero(),
        },
    );
    cache
}

async fn verified_direct_candidate(
    runtime: &AuthorityRuntime,
    transaction: &ckb_types::core::TransactionView,
    command: DirectCommand,
) -> AuthorityDirectVerifiedCandidate {
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture has one free transient compute slot");
    let resolution = match command {
        DirectCommand::Local => runtime.resolve_local_transaction(transaction, execution),
        DirectCommand::TestAccept => {
            runtime.resolve_test_accept_transaction(transaction, execution)
        }
    };
    let request = match resolution.expect("direct resolution has sufficient host resources") {
        AuthorityDirectResolutionOutcome::Verification(request) => request,
        AuthorityDirectResolutionOutcome::Rejected(rejection) => panic!(
            "fixture transaction must reach verification: {:?}",
            rejection.reason().reject()
        ),
    };
    let (_, snapshot) = runtime.paired_chain_for_foundation();
    let cache = completed_cache(&snapshot, transaction);
    let AuthorityDirectVerificationOutcome::Candidate(candidate) = runtime
        .execute_direct_verification(request.bind_cache(&cache), None)
        .await
        .expect("direct verification executes under the captured rules")
    else {
        panic!("fixture transaction must remain a verified candidate")
    };
    candidate
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

fn authority_at(snapshot: &Snapshot) -> TxPoolAuthority {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    authority.force_chain_view(ChainViewId::new(ChainRevision(0), snapshot.tip_hash()));
    authority
}

fn output(capacity: u64) -> CellOutput {
    CellOutput::new_builder()
        .capacity(Capacity::shannons(capacity).pack())
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
    apply_without_work(
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
    let CheckedOutWork::Resolve(work) = committed.into_work().expect("checkout carries work")
    else {
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
    apply_without_work(
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
    let CheckedOutWork::Verify(work) = checkout.into_work().expect("verify work exists") else {
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
    apply_without_work(
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
    apply_without_work(
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
    let CheckedOutWork::Verify(work) = checkout.into_work().expect("verify work exists") else {
        panic!("verify-only checkout must carry verification work")
    };
    let failure = VerificationJob::from_checkout(work, new_snapshot)
        .expect_err("old location evidence must be rejected before VM execution");
    assert_eq!(failure.kind(), ResolutionExecutionKind::StaleView);
    apply_without_work(
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
    apply_without_work(
        authority
            .apply_settlement(settlement)
            .expect("the missing observation is current"),
    );
}

#[test]
fn uak_resolution_reads_only_the_needed_accepted_parent() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let parent = output_tx(803, 1_000, Bytes::new());
    accept_remote_transaction(
        &mut authority,
        parent.clone(),
        83,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let child = spending_tx(804, [OutPoint::new(parent.hash(), 0)], 900);
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
    apply_without_work(
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
    apply_without_work(
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
    let request = job.prepare();
    assert_eq!(expected_key.witness_hash(), &witness_hash);
    let mut cache = init_cache();
    cache.put(
        expected_key,
        Completed {
            cycles: 0,
            fee: Capacity::zero(),
        },
    );

    let execution = request.bind_cache(&cache).execute(None).await;
    assert!(execution.cache_hit);
    assert!(execution.cache_update.is_none());
    apply_without_work(
        authority
            .apply_settlement(execution.settlement)
            .expect("the exact verification capability settles"),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn uak_verification_cache_lookup_cannot_substitute_a_nearby_request() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let cached_tx = TransactionBuilder::default().version(813u32).build();
    let requested_tx = TransactionBuilder::default().version(814u32).build();
    let rules = ScriptVerificationRules::from_env(
        snapshot.consensus(),
        &TxVerifyEnv::new_submit(snapshot.tip_header()),
    );
    let mut cache = init_cache();
    cache.put(
        TxVerificationCacheKey::from_transaction(&cached_tx, rules),
        Completed {
            cycles: 0,
            fee: Capacity::zero(),
        },
    );
    let request =
        checkout_verification_job(&mut authority, Arc::clone(&snapshot), requested_tx, 93)
            .prepare();

    let execution = request.bind_cache(&cache).execute(None).await;
    assert!(!execution.cache_hit);
    assert!(execution.cache_update.is_some());
    apply_without_work(
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
        .evaluate(FeeRate::zero())
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
    let mut cache = init_cache();
    cache.put(
        expected_key,
        Completed {
            cycles: 0,
            fee: Capacity::zero(),
        },
    );

    let DirectVerificationOutcome::Candidate(candidate) = request
        .bind_cache(&cache)
        .execute(None)
        .await
        .expect("the snapshot-bound direct request verifies")
    else {
        panic!("the cached direct verification must produce admission work")
    };
    let (command, work, cache_update, cache_hit) = candidate.into_parts();
    assert_eq!(command, DirectCommand::TestAccept);
    assert!(cache_hit);
    assert!(cache_update.is_none());
    assert_eq!(work.payload().identity().raw, child_key);
    assert!(authority.entry(&child_key).is_none());
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
        .evaluate(FeeRate::zero())
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
    let direct = direct(
        &transaction,
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
        .evaluate(FeeRate::zero())
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
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
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
    let outcome = runtime
        .resolve_test_accept_transaction(&transaction, execution)
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
    let cache = completed_cache(&snapshot, &transaction);
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
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
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
    assert_eq!(completed.cycles, 0);
    assert_eq!(runtime.normalized_snapshot_for_foundation(), before);

    let local = verified_local_candidate(&runtime, &transaction).await;
    let AuthorityDirectAdmissionExecution::Local(local) = runtime
        .settle_verified_direct_admission(local)
        .expect("Local compiles the same policy result into one Apply")
    else {
        panic!("the Local source must preserve Local settlement semantics")
    };
    let (AuthorityLocalAdmissionOutcome::Accepted(local_completed), cache_update, cache_hit) =
        local.into_parts()
    else {
        panic!("the same candidate must commit for Local")
    };
    assert!(cache_hit);
    assert!(cache_update.is_none());
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
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
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
    let completed = Completed {
        cycles: 0,
        fee: Capacity::zero(),
    };
    let rules = ScriptVerificationRules::from_env(
        snapshot.consensus(),
        &TxVerifyEnv::new_submit(snapshot.tip_header()),
    );
    let cache_key = TxVerificationCacheKey::from_transaction(&transaction, rules);

    let test_accept = verified_test_accept_candidate(&runtime, &transaction)
        .await
        .with_cache_update_for_foundation(cache_key, completed);
    let before = runtime.normalized_snapshot_for_foundation();
    assert!(matches!(
        runtime
            .settle_verified_direct_admission(test_accept)
            .expect("TestAccept consumes verification evidence without publication"),
        AuthorityDirectAdmissionExecution::TestAccept(AuthorityTestAcceptOutcome::Accepted(_))
    ));
    assert_eq!(runtime.normalized_snapshot_for_foundation(), before);

    let local = verified_local_candidate(&runtime, &transaction)
        .await
        .with_cache_update_for_foundation(cache_key, completed);
    let AuthorityDirectAdmissionExecution::Local(local) = runtime
        .settle_verified_direct_admission(local)
        .expect("Local accepts and unlocks the post-commit cache consequence")
    else {
        panic!("the Local source must preserve Local settlement semantics")
    };
    let (outcome, cache_update, cache_hit) = local.into_parts();
    assert!(matches!(
        outcome,
        AuthorityLocalAdmissionOutcome::Accepted(_)
    ));
    assert!(!cache_hit);
    assert_eq!(
        cache_update.map(|update| update.into_parts()),
        Some((cache_key, completed))
    );

    let duplicate = verified_local_candidate(&runtime, &transaction)
        .await
        .with_cache_update_for_foundation(cache_key, completed);
    let AuthorityDirectAdmissionExecution::Local(duplicate) = runtime
        .settle_verified_direct_admission(duplicate)
        .expect("an Accepted duplicate is a committed acknowledgement only")
    else {
        panic!("the Local source must preserve Local settlement semantics")
    };
    let (outcome, cache_update, _) = duplicate.into_parts();
    assert!(matches!(
        outcome,
        AuthorityLocalAdmissionOutcome::Duplicate(_)
    ));
    assert!(cache_update.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn uak_test_accept_treats_every_owner_phase_as_a_read_only_duplicate() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
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
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
        .expect("the production runtime fixture is valid");
    let parent = output_tx(824, 1_000, Bytes::new());
    let transaction = spending_tx(0, [OutPoint::new(parent.hash(), 0)], 900);
    let execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture has one free transient compute slot");
    let AuthorityDirectResolutionOutcome::Rejected(rejection) = runtime
        .resolve_local_transaction(&transaction, execution)
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
fn uak_stable_direct_rejection_is_read_only_for_test_accept_and_atomic_for_local() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
        .expect("the production runtime fixture is valid");
    let transaction = TransactionBuilder::default().version(1u32).build();

    let test_execution = runtime
        .try_compute_execution_for_foundation()
        .expect("the fixture has one free transient compute slot");
    let AuthorityDirectResolutionOutcome::Rejected(test_rejection) = runtime
        .resolve_test_accept_transaction(&transaction, test_execution)
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
    let AuthorityDirectResolutionOutcome::Rejected(local_rejection) = runtime
        .resolve_local_transaction(&transaction, local_execution)
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
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), snapshot.clone())
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
        _,
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
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
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
