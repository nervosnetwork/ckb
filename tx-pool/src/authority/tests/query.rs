use super::super::{
    plan::TxPoolAuthority,
    query::{
        AuthorityQueryError, AuthorityQueryScratch, AuthorityTransactionLookup,
        AuthorityTransactionStatusLookup, FeeEstimateReadError, FullQueryCapture, PublicPoolStatus,
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
    prelude::{Entity, Pack},
};
use std::{collections::HashSet, sync::Arc, time::Duration};

fn runtime_with(authority: TxPoolAuthority) -> AuthorityRuntime {
    runtime_with_snapshot(authority, genesis_snapshot())
}

fn runtime_with_snapshot(authority: TxPoolAuthority, snapshot: Arc<Snapshot>) -> AuthorityRuntime {
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
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

#[tokio::test]
async fn uak_owned_transaction_query_hides_replacement_history_and_reports_minimum_fee() {
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
    let plan = authority
        .plan_accept_for_foundation(&replacement, version, AcceptedStatus::Pending)
        .expect("replacement disposition plans");
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

    let ids = runtime.pool_ids().await.expect("pool IDs remain coherent");
    assert_eq!(ids.pending, vec![replacement_tx.hash()]);
    assert!(ids.proposed.is_empty());
    let info = runtime
        .all_entry_info()
        .await
        .expect("entry info exposes the retained conflict history");
    assert!(info.pending.contains_key(&replacement_tx.hash()));
    assert!(info.proposed.is_empty());
    assert_eq!(info.conflicted, vec![victim_tx.hash()]);
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
fn uak_relay_cycle_query_cannot_alias_a_colliding_proposal_id() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = TransactionBuilder::default()
        .version(815u32)
        .output(CellOutput::default())
        .output_data(Bytes::from_static(b"raw-owner").pack())
        .build();
    accept_remote_transaction(
        &mut authority,
        transaction.clone(),
        815,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let runtime = runtime_with(authority);
    let raw = transaction.hash();
    let mut alternate_bytes = raw.as_slice().to_vec();
    alternate_bytes[31] ^= 1;
    let alternate = Byte32::from_slice(&alternate_bytes).expect("the alternate hash is fixed-size");
    assert_eq!(
        ckb_types::packed::ProposalShortId::from_tx_hash(&raw),
        ckb_types::packed::ProposalShortId::from_tx_hash(&alternate)
    );

    assert!(matches!(
        runtime
            .transaction_lookup(&alternate)
            .expect("the colliding raw-hash miss is a coherent query"),
        AuthorityTransactionLookup::RecentRejectFallback
    ));
    let AuthorityTransactionLookup::Live(found) = runtime
        .transaction_lookup(&raw)
        .expect("the exact raw owner is a coherent query")
    else {
        panic!("the exact raw owner must remain visible");
    };
    assert_eq!(found.transaction.hash(), raw);

    assert!(
        runtime
            .live_cell_receipt(OutPoint::new(alternate.clone(), 0))
            .resolve(false)
            .is_unknown(),
        "a proposal-index hit cannot fabricate an output for another raw hash"
    );
    assert!(
        runtime
            .live_cell_receipt(OutPoint::new(raw.clone(), 0))
            .resolve(false)
            .is_live(),
        "the exact raw producer remains visible to the live-cell overlay"
    );

    assert!(
        runtime
            .accepted_with_cycles(vec![alternate])
            .expect("a full-hash miss is a coherent empty observation")
            .is_empty()
    );
    assert_eq!(
        runtime
            .accepted_with_cycles(vec![raw])
            .expect("the exact raw owner is observable"),
        vec![(transaction, 0)]
    );
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
fn uak_fresh_proposal_filter_excludes_accepted_and_preaccepted_owners() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let accepted = tx(811);
    accept_remote_transaction(
        &mut authority,
        accepted.clone(),
        811,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let preaccepted = tx(813);
    admit_remote(&mut authority, 813, 813);
    let unknown = tx(814).proposal_short_id();
    let runtime = runtime_with(authority);

    let fresh = runtime
        .filter_fresh_proposals(vec![
            accepted.proposal_short_id(),
            preaccepted.proposal_short_id(),
            unknown.clone(),
        ])
        .expect("fresh proposal filtering is coherent");
    assert_eq!(fresh, vec![unknown]);
}

#[tokio::test]
async fn uak_storage_receipts_release_authority_before_storage_lookup() {
    let fixture = overlay_fixture();
    let spent_receipt = fixture.runtime.live_cell_receipt(fixture.spent);
    let live_receipt = fixture.runtime.live_cell_receipt(fixture.live);
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
        .await
        .expect("later clear applies independently");

    assert!(spent_receipt.resolve(true).is_unknown());
    assert!(live_receipt.resolve(true).is_live());
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

#[tokio::test]
async fn uak_fee_receipt_is_owned_and_mutation_independent() {
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
        .await
        .expect("fee receipt captures one authority cut");
    runtime
        .clear_pool(genesis_snapshot())
        .await
        .expect("later clear is independent");

    assert_eq!(
        fee.estimate(crate::constants::MIN_ESTIMATE_TARGET)
            .expect("valid target estimates"),
        FeeRate::zero()
    );

    let empty_runtime = runtime_with(TxPoolAuthority::for_foundation(limits()));
    let invalid = empty_runtime
        .fee_estimate_receipt()
        .await
        .expect("empty receipt is coherent")
        .estimate(crate::constants::MIN_ESTIMATE_TARGET - 1);
    assert_eq!(invalid, Err(FeeEstimateReadError::InvalidTarget));
}

#[tokio::test]
async fn uak_full_query_capacity_is_derived_from_the_captured_cut() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = accept_remote_transaction(
        &mut authority,
        tx(853),
        853,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let scratch = AuthorityQueryScratch::new(2);
    let mut permit = scratch.acquire().await;

    let view = authority.read_view();
    let required = match permit
        .capture_pool_ids(&view)
        .expect("the coherent cut is structurally valid")
    {
        FullQueryCapture::NeedsGrow(required) => required,
        FullQueryCapture::Prepared(_) => panic!("zero-capacity scratch cannot hold one owner"),
    };
    drop(view);
    assert_eq!(required, 1);

    permit
        .grow(required)
        .expect("scratch grows only after releasing the authority cut");
    let view = authority.read_view();
    let prepared = match permit
        .capture_pool_ids(&view)
        .expect("the retried coherent cut is structurally valid")
    {
        FullQueryCapture::Prepared(prepared) => prepared,
        FullQueryCapture::NeedsGrow(_) => panic!("the grown scratch covers this cut"),
    };
    drop(view);
    let ids = prepared.finish().expect("the captured rows materialize");
    assert_eq!(ids.pending, vec![hash.0]);
}

#[tokio::test]
async fn uak_full_query_scratch_rejects_bounds_and_allocation_failure() {
    let bounded = AuthorityQueryScratch::new(2);
    let mut permit = bounded.acquire().await;
    assert_eq!(permit.grow(3), Err(AuthorityQueryError::Projection));
    drop(permit);

    let impossible = AuthorityQueryScratch::new(usize::MAX);
    let mut permit = impossible.acquire().await;
    permit.grow(1).expect("the first bounded growth succeeds");
    let first_capacity = permit.prepared_rows_for_foundation();
    permit
        .grow(first_capacity.checked_add(1).expect("one more row fits"))
        .expect("repeated growth makes strict progress");
    assert!(permit.prepared_rows_for_foundation() > first_capacity);
    assert_eq!(
        permit.grow(usize::MAX),
        Err(AuthorityQueryError::Allocation)
    );
}

#[tokio::test]
async fn uak_full_query_gate_does_not_serialize_point_status_reads() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(851);
    let hash = accept_remote_transaction(
        &mut authority,
        transaction,
        851,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let runtime = runtime_with(authority);
    let permit = runtime.acquire_full_query_for_foundation().await;

    assert!(matches!(
        runtime.transaction_status_lookup(&hash.0),
        AuthorityTransactionStatusLookup::Live(_)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), runtime.pool_ids())
            .await
            .is_err(),
        "a second full scan waits for the sole full-query permit"
    );

    drop(permit);
    let ids = tokio::time::timeout(Duration::from_secs(1), runtime.pool_ids())
        .await
        .expect("releasing the permit wakes the queued full scan")
        .expect("the resumed full scan is coherent");
    assert_eq!(ids.pending, vec![hash.0]);
}
