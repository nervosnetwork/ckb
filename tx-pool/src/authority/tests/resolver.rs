use super::foundation::{accept_remote_transaction, apply_without_work, limits, owner_version};
use crate::authority::{
    plan::TxPoolAuthority,
    resolver::{
        ResolutionEvaluation, ResolutionExecutionKind, ResolutionJob, ResolutionProbeObservation,
        VerificationJob,
    },
    state::{
        AcceptedStatus, ChainRevision, ChainViewId, DependencyKey, OwnedTx, PreAcceptedPhase,
        QueuedWork, ValidatedAdmission, VerifyCapability, WorkPermit,
    },
    work::CheckedOutWork,
};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_network::PeerIndex;
use ckb_script::TxVerifyEnv;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    U256,
    bytes::Bytes,
    core::{Capacity, DepType, FeeRate, TransactionBuilder},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint, OutPointVec},
    prelude::{Builder, Entity, Pack, Unpack},
};
use ckb_verification::cache::{Completed, ScriptVerificationRules};
use std::sync::Arc;

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
    let job = checkout_verification_job(&mut authority, Arc::clone(&snapshot), tx, 92);
    let request = job.prepare();
    let expected_rules = ScriptVerificationRules::from_env(
        snapshot.consensus(),
        &TxVerifyEnv::new_submit(snapshot.tip_header()),
    );
    assert_eq!(request.cache_key().witness_hash(), &witness_hash);
    assert_eq!(request.cache_key().script_rules(), expected_rules);

    let execution = request
        .execute(
            Some(Completed {
                cycles: 0,
                fee: Capacity::zero(),
            }),
            None,
        )
        .await
        .expect("the exact cached proof revalidates under the paired snapshot");
    assert!(execution.cache_hit);
    assert!(execution.cache_update.is_none());
    apply_without_work(
        authority
            .apply_settlement(execution.settlement)
            .expect("the exact verification capability settles"),
    );
}
