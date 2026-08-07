use super::super::{
    plan::{CandidateDispositionPlan, TxPoolAuthority},
    query::{
        AuthorityTransactionLookup, AuthorityTransactionStatusLookup, FeeEstimateReadError,
        PublicPoolStatus,
    },
    runtime::AuthorityRuntime,
    state::{AcceptedStatus, RawTxHash},
};
use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, admit_remote,
    genesis_snapshot, limits, owner_version, resolved_payload_with_facts, runtime_config, tx,
    verify_remote_transaction, verify_remote_transaction_with_payload,
};
use ckb_proposal_table::ProposalView;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, FeeRate, TransactionBuilder},
    packed::{Byte32, CellInput, CellOutput, OutPoint},
    prelude::Pack,
};
use std::{collections::HashSet, sync::Arc};

fn runtime_with(authority: TxPoolAuthority) -> AuthorityRuntime {
    runtime_with_snapshot(authority, genesis_snapshot())
}

fn runtime_with_snapshot(authority: TxPoolAuthority, snapshot: Arc<Snapshot>) -> AuthorityRuntime {
    let runtime = AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
        .expect("the production runtime fixture is valid");
    runtime.with_authority_for_foundation(|slot| *slot = authority);
    runtime
}

fn snapshot_with_proposed(proposal: ckb_types::packed::ProposalShortId) -> Arc<Snapshot> {
    let base = genesis_snapshot();
    let store = MockStore::default();
    Arc::new(Snapshot::new(
        base.tip_header().clone(),
        base.total_difficulty().clone(),
        base.epoch_ext().clone(),
        store.store().get_snapshot(),
        ProposalView::new(HashSet::new(), HashSet::from([proposal])),
        base.cloned_consensus(),
    ))
}

struct OverlayFixture {
    runtime: AuthorityRuntime,
    parent_tx: ckb_types::core::TransactionView,
    parent: RawTxHash,
    preaccepted_tx: ckb_types::core::TransactionView,
    spent: OutPoint,
    live: OutPoint,
}

fn overlay_fixture() -> OverlayFixture {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = TransactionBuilder::default()
        .version(821u32)
        .output(CellOutput::default())
        .output_data(Bytes::from_static(b"spent").pack())
        .output(CellOutput::default())
        .output_data(Bytes::from_static(b"live").pack())
        .build();
    let parent = accept_remote_transaction(
        &mut authority,
        parent_tx.clone(),
        821,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let spent = OutPoint::new(parent_tx.hash(), 0);
    let live = OutPoint::new(parent_tx.hash(), 1);
    let child_tx = TransactionBuilder::default()
        .version(822u32)
        .input(CellInput::new(spent.clone(), 0))
        .build();
    accept_remote_transaction(
        &mut authority,
        child_tx,
        822,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let preaccepted_tx = tx(823);
    admit_remote(&mut authority, 823, 823);
    OverlayFixture {
        runtime: runtime_with(authority),
        parent_tx,
        parent,
        preaccepted_tx,
        spent,
        live,
    }
}

#[test]
fn uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee() {
    let rate = FeeRate::from_u64(1_000);
    let mut authority = TxPoolAuthority::with_replacement(limits(), rate);
    let chain_input = OutPoint::new(Byte32::new([81; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(801u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        801,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![chain_input.clone()],
            Capacity::shannons(1_000),
        ),
    );
    let snapshot = genesis_snapshot();
    let before =
        super::super::query::transaction_lookup(&authority.read_view(), &snapshot, &victim)
            .expect("accepted query compiles");
    let AuthorityTransactionLookup::Live(before) = before else {
        panic!("accepted membership must be publicly live");
    };
    assert_eq!(before.status, PublicPoolStatus::Pending);
    assert!(before.min_replace_fee.is_some());

    let replacement_tx = TransactionBuilder::default()
        .version(802u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx.clone(),
        802,
        resolved_payload_with_facts(
            &replacement_tx,
            Vec::new(),
            vec![chain_input],
            Capacity::shannons(10_000),
        ),
    );
    let version = owner_version(&authority, &replacement);
    let disposition = authority
        .plan_candidate_disposition_for_foundation(&replacement, version, AcceptedStatus::Pending)
        .expect("replacement disposition plans");
    let CandidateDispositionPlan::Accepted(plan) = disposition else {
        panic!("sufficient replacement fee must produce an accepted disposition");
    };
    drop(plan.apply());

    let runtime = runtime_with(authority);
    assert!(matches!(
        runtime
            .transaction_lookup(&victim.0)
            .expect("history lookup is coherent"),
        AuthorityTransactionLookup::RecentRejectFallback
    ));
    let AuthorityTransactionLookup::Live(replacement) = runtime
        .transaction_lookup(&replacement.0)
        .expect("winner lookup is coherent")
    else {
        panic!("replacement winner must be publicly live");
    };
    assert_eq!(replacement.transaction.hash(), replacement_tx.hash());
    assert_eq!(replacement.status, PublicPoolStatus::Pending);
}

#[test]
fn uak_status_and_detail_queries_isolate_optional_replacement_fee_overflow() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let transaction = tx(803);
    let hash = accept_remote_transaction_with_payload(
        &mut authority,
        transaction.clone(),
        803,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &transaction,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(u64::MAX),
        ),
    );
    let runtime = runtime_with(authority);

    let AuthorityTransactionStatusLookup::Live(status) = runtime.transaction_status_lookup(&hash.0)
    else {
        panic!("accepted membership must remain visible to the status query");
    };
    assert_eq!(status.status, PublicPoolStatus::Pending);

    let AuthorityTransactionLookup::Live(detail) = runtime
        .transaction_lookup(&hash.0)
        .expect("optional replacement-fee overflow is not an authority fault")
    else {
        panic!("accepted membership must remain visible to the detail query");
    };
    assert_eq!(detail.transaction.hash(), transaction.hash());
    assert_eq!(detail.fee, Some(Capacity::shannons(u64::MAX)));
    assert_eq!(detail.min_replace_fee, None);
}

#[test]
fn uak_owned_pool_queries_share_one_status_and_aggregate_cut() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let pending_tx = tx(811);
    let pending = accept_remote_transaction(
        &mut authority,
        pending_tx.clone(),
        811,
        AcceptedStatus::Gap,
        Vec::new(),
    );
    let proposed_tx = tx(812);
    let proposed = accept_remote_transaction(
        &mut authority,
        proposed_tx.clone(),
        812,
        AcceptedStatus::Proposed,
        Vec::new(),
    );
    let preaccepted_tx = tx(813);
    let preaccepted = admit_remote(&mut authority, 813, 813);
    let runtime = runtime_with(authority);

    let summary = runtime.pool_summary().expect("summary is coherent");
    assert_eq!((summary.pending_size, summary.proposed_size), (1, 1));
    assert_eq!(
        summary.total_tx_size,
        pending_tx.data().serialized_size_in_block()
            + proposed_tx.data().serialized_size_in_block()
    );
    assert_eq!(summary.orphan_size, 0);

    let ids = runtime.pool_ids().expect("ids are coherent");
    assert_eq!(ids.pending, vec![pending.0.clone()]);
    assert_eq!(ids.proposed, vec![proposed.0.clone()]);
    let info = runtime.all_entry_info().expect("entry info is coherent");
    assert!(info.pending.contains_key(&pending.0));
    assert!(info.proposed.contains_key(&proposed.0));
    assert!(info.conflicted.is_empty());

    let detail = runtime
        .pool_detail(&pending.0)
        .expect("detail query is coherent")
        .expect("accepted entry has detail");
    assert_eq!(detail.entry_status, "gap");
    assert_eq!(detail.pending_count, 1);
    assert_eq!(detail.proposed_count, 1);
    assert_eq!(detail.rank_in_pending, 1);
    assert!(
        runtime
            .pool_detail(&preaccepted.0)
            .expect("preaccepted lookup is coherent")
            .is_none()
    );

    let cycles = runtime
        .accepted_with_cycles(vec![
            pending_tx.proposal_short_id(),
            proposed_tx.proposal_short_id(),
            preaccepted_tx.proposal_short_id(),
        ])
        .expect("accepted cycles are coherent");
    assert_eq!(cycles.len(), 2);
    let fresh = runtime
        .filter_fresh_proposals(vec![
            pending_tx.proposal_short_id(),
            proposed_tx.proposal_short_id(),
            preaccepted_tx.proposal_short_id(),
            tx(814).proposal_short_id(),
        ])
        .expect("fresh proposal filter is coherent");
    assert_eq!(fresh, vec![tx(814).proposal_short_id()]);
}

#[test]
fn uak_resolved_preaccepted_query_uses_current_proposal_window() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(815);
    let hash = verify_remote_transaction(&mut authority, transaction.clone(), 815, Vec::new());
    let snapshot = snapshot_with_proposed(transaction.proposal_short_id());
    let runtime = runtime_with_snapshot(authority, snapshot);

    let AuthorityTransactionLookup::Live(found) = runtime
        .transaction_lookup(&hash.0)
        .expect("resolved preaccepted lookup is coherent")
    else {
        panic!("resolved preaccepted owner remains publicly live");
    };
    assert_eq!(found.status, PublicPoolStatus::Proposed);
    assert_eq!(found.transaction.hash(), transaction.hash());
    assert_eq!(found.cycles, Some(0));
}

#[test]
fn uak_live_cell_receipt_releases_authority_before_storage_lookup() {
    let fixture = overlay_fixture();
    let spent_receipt = fixture.runtime.live_cell_receipt(fixture.spent);
    let live_receipt = fixture.runtime.live_cell_receipt(fixture.live);
    fixture
        .runtime
        .clear_pool(genesis_snapshot())
        .expect("later clear applies independently");

    assert!(spent_receipt.resolve(true).is_unknown());
    assert!(live_receipt.resolve(true).is_live());
}

#[test]
fn uak_compact_receipt_releases_authority_before_storage_lookup() {
    let fixture = overlay_fixture();
    let compact = fixture
        .runtime
        .capture_compact_block(vec![
            fixture.parent_tx.proposal_short_id(),
            fixture.preaccepted_tx.proposal_short_id(),
        ])
        .expect("compact receipt captures every retrievable owner");
    fixture
        .runtime
        .clear_pool(genesis_snapshot())
        .expect("later clear applies independently");

    let compact = compact
        .resolve()
        .expect("materialization needs no authority guard");
    assert_eq!(compact.len(), 2);
    assert_eq!(
        compact[&fixture.parent_tx.proposal_short_id()].hash(),
        fixture.parent.0
    );
    assert_eq!(
        compact[&fixture.preaccepted_tx.proposal_short_id()].hash(),
        fixture.preaccepted_tx.hash()
    );
}

#[test]
fn uak_persistence_receipt_is_owned_and_mutation_independent() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let accepted_tx = tx(831);
    accept_remote_transaction(
        &mut authority,
        accepted_tx.clone(),
        831,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    admit_remote(&mut authority, 832, 832);
    let runtime = runtime_with(authority);

    let persistence = runtime
        .persistence_receipt()
        .expect("persistence receipt captures one authority cut");
    runtime
        .clear_pool(genesis_snapshot())
        .expect("later clear is independent");

    let parent_first = persistence
        .into_parent_first()
        .expect("captured relations are acyclic");
    assert_eq!(parent_first.accepted().len(), 1);
    assert!(parent_first.recovery().is_empty());
    let (accepted, recovery) = parent_first.into_transactions();
    assert_eq!(accepted[0].hash(), accepted_tx.hash());
    assert!(recovery.is_empty());
}

#[test]
fn uak_fee_receipt_is_owned_and_mutation_independent() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    accept_remote_transaction(
        &mut authority,
        tx(841),
        841,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let runtime = runtime_with(authority);
    let fee = runtime
        .fee_estimate_receipt()
        .expect("fee receipt captures one authority cut");
    runtime
        .clear_pool(genesis_snapshot())
        .expect("later clear is independent");

    assert_eq!(
        fee.estimate(crate::constants::MIN_ESTIMATE_TARGET)
            .expect("valid target estimates"),
        FeeRate::zero()
    );

    let empty_runtime = runtime_with(TxPoolAuthority::for_foundation(limits()));
    let invalid = empty_runtime
        .fee_estimate_receipt()
        .expect("empty receipt is coherent")
        .estimate(crate::constants::MIN_ESTIMATE_TARGET - 1);
    assert_eq!(invalid, Err(FeeEstimateReadError::InvalidTarget));
}
