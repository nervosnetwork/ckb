use super::super::{
    plan::TxPoolAuthority,
    read::{AuthorityReadState, AuthorityRpcStatus, PreAcceptedReadPhase},
    state::{
        AcceptedAtMillis, AcceptedStatus, PoolGeneration, PreAcceptedSource, RawTxHash,
        ValidatedAdmission,
    },
};
use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, admit_remote, apply_plan,
    genesis_snapshot, limits, owner_version, resolved_payload_with_facts, tx,
};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder, TransactionView},
    packed::{CellInput, CellOutput, OutPoint},
    prelude::Pack,
};

#[derive(Debug, PartialEq, Eq)]
struct QueryCut {
    state: AuthorityReadState,
    rpc_status: Option<AuthorityRpcStatus>,
    pending: Vec<RawTxHash>,
    proposed: Vec<RawTxHash>,
    accepted_gap: usize,
    accepted_proposed: usize,
}

fn materialize_query(authority: &TxPoolAuthority, hash: &RawTxHash) -> QueryCut {
    let view = authority.read_view();
    let snapshot = genesis_snapshot();
    let entry = view.entry_by_raw(hash).expect("fixture owner is visible");
    let ids = view.pool_ids().expect("derived status counts are coherent");
    let summary = view.summary().expect("derived owner counts are coherent");
    QueryCut {
        state: entry.state(),
        rpc_status: entry.rpc_status(&snapshot),
        pending: ids.pending,
        proposed: ids.proposed,
        accepted_gap: summary.accepted_gap,
        accepted_proposed: summary.accepted_proposed,
    }
}

#[test]
fn uak_query_never_splices_two_authority_cuts() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(701);
    let hash = accept_remote_transaction(
        &mut authority,
        transaction.clone(),
        1,
        AcceptedStatus::Gap,
        Vec::new(),
    );

    let before = {
        let view = authority.read_view();
        assert_eq!(view.owner_count(), 1);
        let raw = view
            .entry_by_raw(&hash)
            .expect("raw lookup finds the owner");
        assert_eq!(raw.identity().raw, hash);
        assert_eq!(raw.transaction().hash(), transaction.hash());
        assert_eq!(raw.version(), owner_version(&authority, &hash));
        assert_eq!(raw.arrival().0, 0);
        assert_eq!(raw.fee(), Some(Capacity::shannons(1)));
        assert_eq!(raw.cycles(), Some(0));
        assert_eq!(raw.accepted_at(), Some(AcceptedAtMillis::FOUNDATION));
        let proposal = raw.identity().proposal.clone();
        let by_proposal = view
            .entry_by_proposal(&proposal)
            .expect("proposal index is coherent")
            .expect("proposal index finds the owner");
        assert_eq!(by_proposal.identity().raw, hash);
        let compact = view
            .compact_transactions(std::slice::from_ref(&proposal.0))
            .expect("compact lookup is one coherent cut");
        let first = compact.first().expect("the requested transaction exists");
        assert_eq!(compact.len(), 1);
        assert_eq!(first.1.hash(), transaction.hash());
        assert!(
            view.replacement_history_hashes()
                .expect("conflict projection is valid")
                .is_empty()
        );
        materialize_query(&authority, &hash)
    };

    let version = owner_version(&authority, &hash);
    apply_plan(
        authority
            .plan_status_for_foundation(&hash, version, AcceptedStatus::Proposed)
            .expect("status transition plans"),
    );
    let after = materialize_query(&authority, &hash);

    assert_eq!(
        before.state,
        AuthorityReadState::Accepted(AcceptedStatus::Gap)
    );
    assert_eq!(before.rpc_status, Some(AuthorityRpcStatus::Pending));
    assert_eq!(before.pending, vec![hash.clone()]);
    assert!(before.proposed.is_empty());
    assert_eq!((before.accepted_gap, before.accepted_proposed), (1, 0));

    assert_eq!(
        after.state,
        AuthorityReadState::Accepted(AcceptedStatus::Proposed)
    );
    assert_eq!(after.rpc_status, Some(AuthorityRpcStatus::Proposed));
    assert!(after.pending.is_empty());
    assert_eq!(after.proposed, vec![hash]);
    assert_eq!((after.accepted_gap, after.accepted_proposed), (0, 1));
}

#[test]
fn uak_read_view_keeps_unaccepted_payloads_visible_without_fabricating_proof() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(702);
    let hash = admit_remote(&mut authority, 702, 2);
    let proposal = authority
        .entry(&hash)
        .expect("fixture owner exists")
        .record()
        .identity
        .proposal
        .clone();
    let view = authority.read_view();
    let entry = view.entry_by_raw(&hash).expect("queued owner is visible");
    assert_eq!(entry.transaction().hash(), transaction.hash());
    assert_eq!(
        entry.state(),
        AuthorityReadState::PreAccepted(PreAcceptedReadPhase::ResolveQueued)
    );
    assert_eq!(
        entry.rpc_status(&genesis_snapshot()),
        Some(AuthorityRpcStatus::Pending)
    );
    assert_eq!(entry.fee(), None);
    assert_eq!(entry.cycles(), None);
    assert_eq!(entry.accepted_at(), None);
    assert!(
        view.pool_ids()
            .expect("accepted IDs are coherent")
            .pending
            .is_empty()
    );
    let summary = view.summary().expect("owner summary is coherent");
    assert_eq!(summary.owners, 1);
    assert_eq!(summary.preaccepted, 1);
    assert_eq!(summary.queued, 1);
    assert_eq!(summary.computing, 0);
    assert_eq!(summary.waiting_missing, 0);
    assert_eq!(summary.replacement_history, 0);
    assert_eq!(summary.ready, 0);
    let compact = view
        .compact_transactions(std::slice::from_ref(&proposal.0))
        .expect("compact lookup includes every owner phase");
    let first = compact.first().expect("the requested transaction exists");
    assert_eq!(compact.len(), 1);
    assert_eq!(first.1.hash(), transaction.hash());
}

fn dependent_transactions(
    parent_version: u32,
    child_version: u32,
) -> (TransactionView, TransactionView) {
    let parent = TransactionBuilder::default()
        .version(parent_version)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let child = TransactionBuilder::default()
        .version(child_version)
        .input(CellInput::new(OutPoint::new(parent.hash(), 0), 0))
        .build();
    (parent, child)
}

fn hashes(transactions: &[std::sync::Arc<TransactionView>]) -> Vec<ckb_types::packed::Byte32> {
    transactions
        .iter()
        .map(|transaction| transaction.hash())
        .collect()
}

#[test]
fn uak_persistence_receipt_is_coherent_and_parent_first() {
    let mut authority = TxPoolAuthority::for_foundation(limits());

    // The child enters accepted membership first while its input is chain
    // sourced. Adding the detached parent later rewires the exact causal
    // projection, so arrival order is deliberately the reverse of dependency
    // order.
    let (accepted_parent, accepted_child) = dependent_transactions(703, 704);
    let accepted_parent_output = OutPoint::new(accepted_parent.hash(), 0);
    let child_payload = resolved_payload_with_facts(
        &accepted_child,
        Vec::new(),
        vec![accepted_parent_output],
        Capacity::shannons(2),
    );
    let accepted_child_hash = accept_remote_transaction_with_payload(
        &mut authority,
        accepted_child.clone(),
        3,
        AcceptedStatus::Pending,
        child_payload,
    );
    let accepted_parent_hash = accept_remote_transaction(
        &mut authority,
        accepted_parent.clone(),
        4,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    assert_eq!(
        authority.accepted_parents(&accepted_child_hash),
        Some(std::collections::HashSet::from([accepted_parent_hash]))
    );

    // Recovery also arrives child first. Raw cell dependencies are sufficient
    // for restart ordering; no old proof or membership status is exported.
    let (recovery_parent, recovery_child) = dependent_transactions(705, 706);
    for transaction in [recovery_child.clone(), recovery_parent.clone()] {
        let admission = ValidatedAdmission::recovery(transaction, PoolGeneration(0))
            .expect("fixture recovery admission is valid");
        apply_plan(
            authority
                .plan_admission(admission)
                .expect("fixture recovery admission plans"),
        );
    }

    // Volatile Remote and Proposal owners are intentionally excluded.
    let _remote = admit_remote(&mut authority, 707, 5);
    let proposal =
        ValidatedAdmission::proposal(tx(708)).expect("fixture proposal admission is valid");
    apply_plan(
        authority
            .plan_admission(proposal)
            .expect("fixture proposal admission plans"),
    );

    let receipt = authority
        .read_view()
        .capture_persistence()
        .expect("one authority cut captures persistence rows");
    assert_eq!(receipt.selected_len(), 4);

    // A later owner cannot enter the already-owned receipt.
    let extra = accept_remote_transaction(
        &mut authority,
        tx(709),
        6,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let after_apply = authority.normalized_snapshot();
    let parent_first = receipt
        .into_parent_first()
        .expect("captured causal graphs are acyclic");
    assert_eq!(
        hashes(parent_first.accepted()),
        vec![accepted_parent.hash(), accepted_child.hash()]
    );
    assert_eq!(
        hashes(parent_first.recovery()),
        vec![recovery_parent.hash(), recovery_child.hash()]
    );
    let (accepted, recovery) = parent_first.into_transactions();
    assert_eq!(accepted.len(), 2);
    assert_eq!(recovery.len(), 2);
    assert!(!hashes(&accepted).contains(&extra.0));
    assert_eq!(authority.normalized_snapshot(), after_apply);

    let fresh = authority
        .read_view()
        .capture_persistence()
        .expect("a later read sees the later accepted owner");
    assert_eq!(fresh.selected_len(), 5);
}

#[test]
fn uak_dropped_persistence_receipt_has_no_authority_effect() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    accept_remote_transaction(
        &mut authority,
        tx(710),
        7,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let recovery = ValidatedAdmission::recovery(tx(711), PoolGeneration(0))
        .expect("fixture recovery admission is valid");
    assert!(matches!(recovery.source, PreAcceptedSource::Recovery(_)));
    apply_plan(
        authority
            .plan_admission(recovery)
            .expect("fixture recovery admission plans"),
    );
    let before = authority.normalized_snapshot();
    let receipt = authority
        .read_view()
        .capture_persistence()
        .expect("persistence capture is read-only");
    assert_eq!(receipt.selected_len(), 2);
    drop(receipt);
    assert_eq!(authority.normalized_snapshot(), before);

    let unknown = super::super::state::ProposalId(tx(799).proposal_short_id());
    assert!(
        authority
            .read_view()
            .entry_by_proposal(&unknown)
            .expect("missing proposal is not a projection fault")
            .is_none()
    );
}
