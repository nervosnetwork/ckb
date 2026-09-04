use super::super::{
    chain::{ChainBlockChanges, ProposalWindowPosition, test_support::ChainTransitionFacts},
    chain_boundary::{
        CandidateUncleCollection, ChainBoundaryError, ChainGenerationReplacement,
        ChainUpdateRequest,
    },
    effect::CommittedEffect,
    plan::{
        Backpressure, ComputeSettlementRecovery, PlanError, SharedReadyWaveCompilation, StalePlan,
        TxPoolAuthority,
    },
    resources::{AcceptedResources, ComputeLimits, ResourceLimits, ResourceVector},
    runtime::AuthorityRuntime,
    state::{
        AcceptedStatus, ChainRevision, ChainViewId, DependencyKey, OwnedTx, PreAcceptedPhase,
        PreAcceptedSource, ProposalId, QueuedWork, RawTxHash, ValidatedAdmission, WorkPermit,
    },
    work::CheckedOutWork,
};
use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, admit_remote, apply_plan,
    assert_membership_reference, assert_resource_reference, genesis_snapshot, independent_batch,
    limits, missing_keys, owner_version, resolved_payload_with_facts, runtime_config,
    take_resolve_work, tx, verify_remote_transaction,
};
use ckb_network::PeerIndex;
use ckb_proposal_table::ProposalView;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, Capacity, TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint, ProposalShortId},
    prelude::{Builder, Entity, Pack},
};
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

fn snapshot_with_proposals(
    base: &Snapshot,
    gap: HashSet<ProposalShortId>,
    proposed: HashSet<ProposalShortId>,
) -> Arc<Snapshot> {
    let store = MockStore::default();
    Arc::new(Snapshot::new(
        base.tip_header().clone(),
        base.total_difficulty().clone(),
        base.epoch_ext().clone(),
        store.store().get_snapshot(),
        ProposalView::new(gap, proposed),
        base.cloned_consensus(),
    ))
}

#[test]
fn uak_final_admission_receipt_is_stale_after_chain_view_aba() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let candidate = verify_remote_transaction(&mut authority, tx(67), 67, Vec::new());
    let batch = independent_batch(&authority, std::slice::from_ref(&candidate));
    authority.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::new([67; 32])));
    authority.force_chain_view(ChainViewId::new(ChainRevision(2), Byte32::zero()));
    let before = authority.normalized_snapshot();

    assert!(matches!(
        authority.compile_shared_ready_wave(&batch),
        SharedReadyWaveCompilation::Retry
    ));
    assert_eq!(authority.normalized_snapshot(), before);
    assert_resource_reference(&authority);
}

#[test]
fn uak_chain_tip_not_revision_controls_negative_evidence_freshness() {
    let mut authority = TxPoolAuthority::for_foundation(limits());

    let same_tip = admit_remote(&mut authority, 69, 69);
    let version = owner_version(&authority, &same_tip);
    let checkout = authority
        .checkout_for_foundation(&same_tip, version, WorkPermit::ResolveOnly)
        .expect("same-tip resolve checkout plans");
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    let missing = resolve
        .missing(missing_keys())
        .expect("fixture missing evidence is bounded");
    authority.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::zero()));
    apply_plan(
        authority
            .apply_settlement(missing)
            .expect("same-tip negative evidence remains current"),
    );
    assert!(matches!(
        authority.entry(&same_tip),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Waiting(_))
    ));

    let changed_tip = admit_remote(&mut authority, 70, 70);
    let version = owner_version(&authority, &changed_tip);
    let checkout = authority
        .checkout_for_foundation(&changed_tip, version, WorkPermit::ResolveOnly)
        .expect("changed-tip resolve checkout plans");
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    let missing = resolve
        .missing(missing_keys())
        .expect("fixture missing evidence is bounded");
    authority.force_chain_view(ChainViewId::new(ChainRevision(2), Byte32::new([70; 32])));
    apply_plan(
        authority
            .apply_settlement(missing)
            .expect("matching completion consumes stale negative evidence"),
    );
    assert!(matches!(
        authority.entry(&changed_tip),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_internal_worker_failure_requeues_the_exact_work() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 73, 73);
    let (_, work) = take_resolve_work(
        authority
            .checkout_for_foundation(
                &hash,
                owner_version(&authority, &hash),
                WorkPermit::ResolveOnly,
            )
            .expect("fixture resolve checkout plans"),
    );
    apply_plan(
        authority
            .apply_settlement(work.internal_failure())
            .expect("internal worker failure settles and retries its exact work"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_resource_reference(&authority);
}

fn output_transaction(version: u32) -> TransactionView {
    TransactionBuilder::default()
        .version(version)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build()
}

fn child_transaction(version: u32, parent: &TransactionView) -> TransactionView {
    TransactionBuilder::default()
        .version(version)
        .input(CellInput::new(OutPoint::new(parent.hash(), 0), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build()
}

fn next_view(byte: u8) -> ChainViewId {
    ChainViewId::new(ChainRevision(1), Byte32::new([byte; 32]))
}

fn block_changes(
    attached: Vec<TransactionView>,
    detached: Vec<TransactionView>,
) -> ChainBlockChanges {
    ChainBlockChanges::for_foundation(attached, detached, Vec::new(), Vec::new())
}

fn park_missing(authority: &mut TxPoolAuthority, hash: &RawTxHash, dependency: DependencyKey) {
    let checkout = authority
        .checkout_for_foundation(
            hash,
            owner_version(authority, hash),
            WorkPermit::ResolveOnly,
        )
        .expect("missing fixture checks out");
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    let settlement = resolve
        .missing(vec![dependency])
        .expect("missing fixture is bounded");
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("missing fixture enters Wait"),
    );
}

fn drain_dependency_maintenance(authority: &mut TxPoolAuthority) {
    drop(
        authority
            .drain_dependency_maintenance_for_foundation()
            .expect("dependency maintenance strictly decreases its rank to zero"),
    );
}

fn large_chain_limits() -> ResourceLimits {
    ResourceLimits::new(
        ResourceVector::new(256, 16 * 1024 * 1024, 4_096, 8),
        ResourceVector::new(256, 16 * 1024 * 1024, 4_096, 8),
        ResourceVector::new(2, 128 * 1024, 32, 2),
        AcceptedResources::new(256, 16 * 1024 * 1024, 16 * 1024 * 1024, 1_000_000),
        ComputeLimits::new(128 * 1024, 128 * 1024, 256),
    )
    .expect("large chain fixture retains the same static compute hierarchy")
}

#[test]
fn uak_chain_commit_closes_a_preaccepted_owner_with_exact_effect_semantics() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = output_transaction(503);
    let admission =
        ValidatedAdmission::remote(transaction.clone(), ckb_network::PeerIndex::from(503))
            .expect("preaccepted committed fixture is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("fixture enters preacceptance"),
    );
    let facts = ChainTransitionFacts::for_foundation(
        next_view(72),
        block_changes(vec![transaction.clone()], Vec::new()),
        Vec::new(),
    )
    .expect("attached preaccepted fact is canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("committed raw owner is selected")
        .validate_for_foundation(Vec::new())
        .expect("commit needs no proposal facts");
    apply_plan(
        authority
            .plan_chain_transition(receipt)
            .expect("owner removal and effect append are atomic"),
    );
    assert!(authority.entry(&hash).is_none());
    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("chain commit queued one exact effect");
    assert_eq!(
        lease.effects(),
        &[CommittedEffect::ChainCommitted {
            tx_hash: RawTxHash(transaction.hash()),
            ingress_peer: ckb_network::PeerIndex::from(503),
        }]
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_chain_conflict_closes_accepted_cell_dep_readers_in_the_same_apply() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let chain_input = OutPoint::new(Byte32::new([76; 32]), 0);
    let provider_tx = TransactionBuilder::default()
        .version(1_317u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let provider_output = OutPoint::new(provider_tx.hash(), 0);
    let provider_payload = resolved_payload_with_facts(
        &provider_tx,
        Vec::new(),
        vec![chain_input.clone()],
        Capacity::shannons(10),
    );
    let provider = accept_remote_transaction_with_payload(
        &mut authority,
        provider_tx,
        1_317,
        AcceptedStatus::Pending,
        provider_payload,
    );
    let reader_tx = TransactionBuilder::default()
        .version(1_318u32)
        .cell_dep(CellDep::new_builder().out_point(provider_output).build())
        .build();
    let reader = accept_remote_transaction(
        &mut authority,
        reader_tx,
        1_318,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let attached = TransactionBuilder::default()
        .version(1_319u32)
        .input(CellInput::new(chain_input, 0))
        .build();
    let facts = ChainTransitionFacts::for_foundation(
        next_view(76),
        block_changes(vec![attached], Vec::new()),
        Vec::new(),
    )
    .expect("cell-dep conflict facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("canonical dependency closure includes accepted cell-dep readers")
        .validate_for_foundation(Vec::new())
        .expect("the closed conflict set needs no proposal facts");
    apply_plan(
        authority
            .plan_chain_transition(receipt)
            .expect("provider and reader terminalize in one atomic Apply"),
    );

    assert!(authority.entry(&provider).is_none());
    assert!(authority.entry(&reader).is_none());
    assert!(authority.primary_projection_consistent());
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_detached_provider_and_accepted_cell_dep_reader_recover_in_one_apply() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let detached_provider = output_transaction(1_320);
    let provider_output = OutPoint::new(detached_provider.hash(), 0);
    let reader_tx = TransactionBuilder::default()
        .version(1_321u32)
        .cell_dep(CellDep::new_builder().out_point(provider_output).build())
        .build();
    let reader = accept_remote_transaction(
        &mut authority,
        reader_tx,
        1_321,
        AcceptedStatus::Proposed,
        Vec::new(),
    );
    let provider = RawTxHash(detached_provider.hash());
    let facts = ChainTransitionFacts::for_foundation(
        next_view(77),
        block_changes(Vec::new(), vec![detached_provider]),
        Vec::new(),
    )
    .expect("detached provider facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("canonical dependency closure includes the accepted cell-dep reader")
        .validate_for_foundation(Vec::new())
        .expect("provider recovery requires no proposal facts");
    apply_plan(
        authority
            .plan_chain_transition(receipt)
            .expect("provider and reader enter recovery in one atomic Apply"),
    );

    let provider_arrival = authority
        .entry(&provider)
        .expect("provider recovered")
        .record()
        .arrival;
    let reader_arrival = authority
        .entry(&reader)
        .expect("cell-dep reader recovered")
        .record()
        .arrival;
    assert!(provider_arrival < reader_arrival);
    for hash in [&provider, &reader] {
        assert!(matches!(
            authority.entry(hash),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.source, PreAcceptedSource::Recovery(_))
                    && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
        ));
    }
    assert_eq!(authority.membership_counts().pending, 0);
    assert!(authority.primary_projection_consistent());
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_rules_transition_cannot_claim_monotonic_accepted_validity() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let contextual_tx = output_transaction(1_315);
    let contextual = verify_remote_transaction(&mut authority, contextual_tx, 1_315, Vec::new());
    apply_plan(
        authority
            .plan_accept_context_sensitive_for_foundation(
                &contextual,
                owner_version(&authority, &contextual),
                AcceptedStatus::Pending,
            )
            .expect("context-sensitive acceptance is validation-derived"),
    );
    let stable = accept_remote_transaction(
        &mut authority,
        output_transaction(1_316),
        1_316,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let facts = ChainTransitionFacts::for_foundation(
        next_view(75),
        block_changes(Vec::new(), Vec::new()),
        Vec::new(),
    )
    .expect("an extension without block deltas is canonical")
    .revalidate_all_for_foundation();
    let receipt = authority
        .chain_validation_work(facts)
        .expect("typed rules invalidation selects every Accepted proof")
        .validate_for_foundation(Vec::new())
        .expect("context recovery requires no proposal facts");
    apply_plan(
        authority
            .plan_chain_transition(receipt)
            .expect("rules transition and recovery commit atomically"),
    );
    assert!(matches!(
        authority.entry(&contextual),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(matches!(
        authority.entry(&stable),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
}

#[tokio::test]
async fn uak_fresh_generation_fallback_preserves_preaccepted_source_and_peer_budget() {
    let constrained = ResourceLimits::new(
        ResourceVector::new(2, 64 * 1024, 64, 1),
        ResourceVector::new(2, 64 * 1024, 64, 1),
        ResourceVector::new(2, 64 * 1024, 64, 1),
        AcceptedResources::new(4, 256 * 1024, 256 * 1024, 1_000),
        ComputeLimits::new(64 * 1024, 64 * 1024, 64),
    )
    .expect("two preaccepted owners and larger accepted membership are valid");
    let mut authority = TxPoolAuthority::for_foundation(constrained);
    let detached_parent = output_transaction(1_759);
    let parent = RawTxHash(detached_parent.hash());
    let child_tx = child_transaction(1_760, &detached_parent);
    let peer = ckb_network::PeerIndex::from(1_760);
    let child_admission =
        ValidatedAdmission::remote(child_tx, peer).expect("remote dependent fixture is valid");
    let child = child_admission.identity.raw.clone();
    let charged = child_admission.charge_for_foundation();
    apply_plan(
        authority
            .plan_admission(child_admission)
            .expect("remote dependent enters preacceptance"),
    );
    let unrelated =
        ValidatedAdmission::remote(tx(1_761), peer).expect("the unrelated remote fixture is valid");
    let unrelated_hash = unrelated.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(unrelated)
            .expect("the second owner fills the live preaccepted budget"),
    );
    let original_source = match authority.entry(&child) {
        Some(OwnedTx::PreAccepted(entry)) => entry.source,
        _ => panic!("the child is a preaccepted owner"),
    };

    let old_generation = authority.generation();
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    runtime.with_authority_for_foundation(|slot| *slot = authority);
    let detached = BlockBuilder::default().transaction(detached_parent).build();
    let command = ChainUpdateRequest::new(
        VecDeque::from([detached]),
        VecDeque::new(),
        Arc::clone(&snapshot),
        CandidateUncleCollection::SkipCandidateUncles,
    )
    .prepare()
    .expect("detached parent facts form one sealed chain command");
    drop(
        runtime
            .apply_chain_update(command)
            .await
            .expect("resource overflow selects the provenance-preserving fresh fallback"),
    );

    runtime.with_authority_for_foundation(|authority| {
        assert!(authority.generation().0 > old_generation.0);
        assert!(matches!(
            authority.entry(&parent),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.source, PreAcceptedSource::Recovery(lease)
                    if lease.generation == authority.generation())
        ));
        assert!(matches!(
            authority.entry(&child),
            Some(OwnedTx::PreAccepted(entry))
                if entry.source == original_source
                    && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
        ));
        assert!(authority.entry(&unrelated_hash).is_none());
        assert_eq!(authority.resources().peer(peer), charged);
        assert_resource_reference(authority);
    });
}

#[test]
fn uak_preaccepted_recovery_does_not_publish_false_input_availability() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let detached_parent = output_transaction(535);
    let unrelated_input = OutPoint::new(Byte32::new([64; 32]), 0);
    let child_tx = TransactionBuilder::default()
        .version(536u32)
        .input(CellInput::new(OutPoint::new(detached_parent.hash(), 0), 0))
        .input(CellInput::new(unrelated_input.clone(), 0))
        .build();
    let child = ValidatedAdmission::remote(child_tx, ckb_network::PeerIndex::from(536))
        .expect("remote dependent fixture is valid");
    apply_plan(
        authority
            .plan_admission(child)
            .expect("remote dependent enters preacceptance"),
    );

    let waiter_tx = TransactionBuilder::default()
        .version(537u32)
        .input(CellInput::new(unrelated_input.clone(), 0))
        .build();
    let waiter = ValidatedAdmission::remote(waiter_tx, ckb_network::PeerIndex::from(537))
        .expect("remote waiter fixture is valid");
    let waiter_hash = waiter.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(waiter)
            .expect("remote waiter enters preacceptance"),
    );
    park_missing(
        &mut authority,
        &waiter_hash,
        DependencyKey::Cell(unrelated_input),
    );
    let waiting_version = owner_version(&authority, &waiter_hash);

    let facts = ChainTransitionFacts::for_foundation(
        next_view(64),
        block_changes(Vec::new(), vec![detached_parent]),
        Vec::new(),
    )
    .expect("detached facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("chain work distinguishes a direct detach from dependent requeue")
        .validate_for_foundation(Vec::new())
        .expect("recovery requires no proposal facts");
    apply_plan(
        authority
            .plan_chain_transition(receipt)
            .expect("chain transition publishes only real dependency changes"),
    );
    drain_dependency_maintenance(&mut authority);

    assert_eq!(owner_version(&authority, &waiter_hash), waiting_version);
    assert!(matches!(
        authority.entry(&waiter_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Waiting(_))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_accepted_chain_conflict_publishes_released_input_availability() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let committed_input = OutPoint::new(Byte32::new([65; 32]), 0);
    let released_input = OutPoint::new(Byte32::new([66; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(538u32)
        .input(CellInput::new(committed_input.clone(), 0))
        .input(CellInput::new(released_input.clone(), 0))
        .build();
    let victim_payload = resolved_payload_with_facts(
        &victim_tx,
        Vec::new(),
        vec![committed_input.clone(), released_input.clone()],
        Capacity::shannons(10),
    );
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx,
        538,
        AcceptedStatus::Pending,
        victim_payload,
    );
    let waiter_tx = TransactionBuilder::default()
        .version(539u32)
        .input(CellInput::new(released_input.clone(), 0))
        .build();
    let waiter = ValidatedAdmission::remote(waiter_tx, ckb_network::PeerIndex::from(539))
        .expect("released-input waiter is valid");
    let waiter_hash = waiter.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(waiter)
            .expect("released-input waiter enters preacceptance"),
    );
    park_missing(
        &mut authority,
        &waiter_hash,
        DependencyKey::Cell(released_input),
    );
    let waiting_version = owner_version(&authority, &waiter_hash);

    let attached = TransactionBuilder::default()
        .version(540u32)
        .input(CellInput::new(committed_input, 0))
        .build();
    let facts = ChainTransitionFacts::for_foundation(
        next_view(65),
        block_changes(vec![attached], Vec::new()),
        Vec::new(),
    )
    .expect("attached conflict facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("accepted conflict closure is bounded")
        .validate_for_foundation(Vec::new())
        .expect("conflict transition requires no proposal facts");
    apply_plan(
        authority
            .plan_chain_transition(receipt)
            .expect("conflict removal and availability publish atomically"),
    );
    drain_dependency_maintenance(&mut authority);

    assert!(authority.entry(&victim).is_none());
    assert_ne!(owner_version(&authority, &waiter_hash), waiting_version);
    assert!(matches!(
        authority.entry(&waiter_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_resource_reference(&authority);
}

#[tokio::test]
async fn uak_runtime_proposal_status_is_independent_of_candidate_uncle_collection() {
    let snapshot = genesis_snapshot();
    let transaction = output_transaction(1_755);
    let proposal = transaction.proposal_short_id();
    let proposed_snapshot =
        snapshot_with_proposals(&snapshot, HashSet::new(), HashSet::from([proposal]));
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let hash = runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(
            authority,
            transaction,
            1_755,
            AcceptedStatus::Pending,
            Vec::new(),
        )
    });
    let command = ChainUpdateRequest::new(
        VecDeque::new(),
        VecDeque::new(),
        Arc::clone(&proposed_snapshot),
        CandidateUncleCollection::SkipCandidateUncles,
    )
    .prepare()
    .expect("empty block facts are a valid chain command");
    let committed = runtime
        .apply_chain_update(command)
        .await
        .expect("the paired snapshot and authority commit in one boundary");

    assert!(committed.candidate_uncles.is_empty());
    assert!(Arc::ptr_eq(&committed.snapshot, &proposed_snapshot));
    runtime.with_authority_for_foundation(|authority| {
        assert!(matches!(
            authority.entry(&hash),
            Some(OwnedTx::Accepted(entry)) if entry.status() == AcceptedStatus::Proposed
        ));
        assert_membership_reference(authority);
    });
}

#[test]
fn uak_chain_request_returns_exact_input_after_preparation_failure() {
    let snapshot = genesis_snapshot();
    let transaction = output_transaction(1_754);
    let invalid = BlockBuilder::default()
        .transaction(transaction.clone())
        .transaction(transaction)
        .build();
    let failure = match ChainUpdateRequest::new(
        VecDeque::new(),
        VecDeque::from([invalid]),
        Arc::clone(&snapshot),
        CandidateUncleCollection::SkipCandidateUncles,
    )
    .prepare()
    {
        Err(failure) => failure,
        Ok(_) => panic!("duplicate block facts must fail canonical preparation"),
    };
    let (error, request) = failure.into_parts();
    assert_eq!(error, ChainBoundaryError::InvalidFacts);
    let (repeated, _request) = match request.prepare() {
        Err(failure) => failure.into_parts(),
        Ok(_) => panic!("the returned request must retain the same exact invalid facts"),
    };
    assert_eq!(repeated, ChainBoundaryError::InvalidFacts);
}

#[tokio::test]
async fn uak_chain_apply_failure_returns_the_same_prepared_command() {
    let snapshot = genesis_snapshot();
    let transaction = child_transaction(0, &output_transaction(1_753));
    let attached = BlockBuilder::default()
        .transaction(transaction.clone())
        .build();
    let command = ChainUpdateRequest::new(
        VecDeque::new(),
        VecDeque::from([attached]),
        Arc::clone(&snapshot),
        CandidateUncleCollection::SkipCandidateUncles,
    )
    .prepare()
    .expect("attached block facts form one prepared command");
    let closed = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the first runtime fixture is valid");
    closed
        .submit_remote_ingress(transaction.clone(), 0, PeerIndex::from(53))
        .expect("the first runtime retains its Remote owner");
    closed
        .close_effects()
        .await
        .expect("the fixture closes effect production before the chain attempt");
    let failure = match closed.apply_chain_update(command).await {
        Err(failure) => failure,
        Ok(_) => panic!("a required relay outcome cannot publish after lifecycle close"),
    };
    let (error, command) = failure.into_parts();
    assert_eq!(error, ChainBoundaryError::LifecycleClosed);

    let open = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the second runtime fixture is valid");
    open.submit_remote_ingress(transaction, 0, PeerIndex::from(53))
        .expect("the second runtime retains its Remote owner");
    drop(
        open.apply_chain_update(command)
            .await
            .expect("the returned command remains complete and commit-capable"),
    );
}

#[tokio::test]
async fn uak_runtime_chain_boundary_commits_compact_hash_cache_with_snapshot() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let transaction = output_transaction(1_756);
    let proposal = transaction.proposal_short_id();
    let raw_hash = transaction.hash();
    let attached = BlockBuilder::default().transaction(transaction).build();
    let attached_hash = attached.hash();
    let command = ChainUpdateRequest::new(
        VecDeque::new(),
        VecDeque::from([attached]),
        Arc::clone(&snapshot),
        CandidateUncleCollection::SkipCandidateUncles,
    )
    .prepare()
    .expect("attached block facts are a valid chain command");
    let committed = runtime
        .apply_chain_update(command)
        .await
        .expect("cache, authority and snapshot share one commit cut");

    assert!(committed.candidate_uncles.is_empty());
    assert_eq!(
        committed
            .attached_blocks
            .front()
            .map(ckb_types::core::BlockView::hash),
        Some(attached_hash),
        "the exact ordered attached block evidence crosses Apply for post-commit observers"
    );
    assert_eq!(
        runtime.committed_hash_for_foundation(&proposal),
        Some(raw_hash)
    );
    let (view, paired_snapshot) = runtime.paired_chain_for_foundation();
    assert_eq!(view.revision(), ChainRevision(1));
    assert_eq!(view.tip().0, paired_snapshot.tip_hash());

    let replacement_snapshot = genesis_snapshot();
    let replacement = runtime
        .apply_chain_generation_replacement(ChainGenerationReplacement::from_snapshot(Arc::clone(
            &replacement_snapshot,
        )))
        .await
        .expect("the minimum chain consequence cannot allocate fallible scratch");
    assert!(replacement.attached_blocks.is_empty());
    assert!(replacement.candidate_uncles.is_empty());
    assert!(Arc::ptr_eq(&replacement.snapshot, &replacement_snapshot));
    assert_eq!(runtime.committed_hash_for_foundation(&proposal), None);
    let (_view, paired_snapshot) = runtime.paired_chain_for_foundation();
    assert!(Arc::ptr_eq(&paired_snapshot, &replacement_snapshot));
}

#[tokio::test]
async fn uak_runtime_chain_boundary_preserves_block_order_for_short_id_collisions() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let proposal = ProposalShortId::new([9; 10]);
    let first = Byte32::new([1; 32]);
    let second = Byte32::new([2; 32]);
    let mut command = ChainUpdateRequest::new(
        VecDeque::new(),
        VecDeque::new(),
        Arc::clone(&snapshot),
        CandidateUncleCollection::SkipCandidateUncles,
    )
    .prepare()
    .expect("empty block facts are a valid chain command");
    command.committed_hashes = vec![
        (proposal.clone(), first),
        (proposal.clone(), second.clone()),
    ];

    let committed = runtime
        .apply_chain_update(command)
        .await
        .expect("cache writes share the ordered chain commit");
    drop(committed);
    assert_eq!(
        runtime.committed_hash_for_foundation(&proposal),
        Some(second)
    );
}

#[tokio::test]
async fn uak_runtime_chain_boundary_converges_an_unrepresentable_recovery_batch() {
    let constrained = ResourceLimits::new(
        ResourceVector::new(1, 64 * 1024, 64, 1),
        ResourceVector::new(1, 64 * 1024, 64, 1),
        ResourceVector::new(1, 64 * 1024, 64, 1),
        AcceptedResources::new(4, 256 * 1024, 256 * 1024, 1_000),
        ComputeLimits::new(64 * 1024, 64 * 1024, 64),
    )
    .expect("one-at-a-time ingress and larger accepted membership are valid");
    let mut authority = TxPoolAuthority::for_foundation(constrained);
    let fenced_peer = PeerIndex::from(1_303);
    drop(
        authority
            .plan_peer_revocation_for_foundation(fenced_peer)
            .expect("the fixture installs one persistent routed peer fence")
            .apply(),
    );
    assert!(authority.peer_is_banned_for_reference(fenced_peer));
    let parent = output_transaction(1_757);
    let child = child_transaction(1_758, &parent);
    let parent_hash = accept_remote_transaction(
        &mut authority,
        parent.clone(),
        1_757,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    accept_remote_transaction(
        &mut authority,
        child.clone(),
        1_758,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let old_generation = authority.generation();
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    runtime.with_authority_for_foundation(|slot| *slot = authority);
    let detached = BlockBuilder::default()
        .transaction(parent)
        .transaction(child)
        .build();
    let command = ChainUpdateRequest::new(
        VecDeque::from([detached]),
        VecDeque::new(),
        Arc::clone(&snapshot),
        CandidateUncleCollection::SkipCandidateUncles,
    )
    .prepare()
    .expect("detached block facts form one sealed chain command");

    let committed = runtime
        .apply_chain_update(command)
        .await
        .expect("the boundary converts detailed resource overflow into a fresh generation");
    drop(committed);
    runtime.with_authority_for_foundation(|authority| {
        assert!(authority.generation().0 > old_generation.0);
        assert_eq!(authority.chain_revision(), ChainRevision(1));
        assert_eq!(authority.owner_count(), 1);
        assert!(authority.entry(&parent_hash).is_some());
        assert!(authority.peer_is_banned_for_reference(fenced_peer));
        assert!(
            authority
                .entries_for_reference()
                .snapshot_for_test()
                .into_iter()
                .all(|(_, owner)| {
                    matches!(
                        owner,
                        OwnedTx::PreAccepted(entry)
                            if matches!(entry.source, PreAcceptedSource::Recovery(lease)
                                if lease.generation == authority.generation())
                    )
                })
        );
        assert_resource_reference(authority);
    });
}

#[test]
fn uak_chain_trusted_proposal_expiry_publishes_definitive_parent_loss() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(1_727);
    let parent_admission =
        ValidatedAdmission::proposal(parent_tx.clone()).expect("trusted proposal fixture is valid");
    let parent = parent_admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(parent_admission)
            .expect("trusted proposal enters preacceptance"),
    );
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let child_admission = ValidatedAdmission::remote(
        child_transaction(1_728, &parent_tx),
        ckb_network::PeerIndex::from(728),
    )
    .expect("dependent child fixture is valid");
    let child = child_admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(child_admission)
            .expect("dependent child enters preacceptance"),
    );
    let (_, child_work) = super::foundation::take_resolve_work(
        authority
            .checkout_for_foundation(
                &child,
                owner_version(&authority, &child),
                WorkPermit::ResolveOnly,
            )
            .expect("dependent child checks out"),
    );
    let proposal = ProposalId(parent_tx.proposal_short_id());
    let facts = ChainTransitionFacts::for_foundation(
        ChainViewId::new(ChainRevision(1), Byte32::zero()),
        block_changes(Vec::new(), Vec::new()),
        vec![proposal.clone()],
    )
    .expect("same-tip proposal transition facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("trusted proposal expiry is bounded")
        .validate_for_foundation(vec![(proposal, ProposalWindowPosition::Outside)])
        .expect("the final proposal position is exhaustive");
    apply_plan(
        authority
            .plan_chain_transition(receipt)
            .expect("parent terminalization and dependency loss are atomic"),
    );

    assert!(authority.entry(&parent).is_none());
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    apply_plan(
        authority
            .apply_settlement(
                child_work
                    .missing(vec![DependencyKey::Cell(parent_output)])
                    .expect("old dependency evidence is bounded"),
            )
            .expect("same-tip completion observes the definitive dependency loss"),
    );
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_chain_trusted_proposal_expiry_invalidates_active_work_without_a_drain() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(1_729);
    let admission = ValidatedAdmission::proposal(transaction.clone())
        .expect("trusted proposal fixture is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("trusted proposal enters preacceptance"),
    );
    let (_, work) = take_resolve_work(
        authority
            .checkout_for_foundation(
                &hash,
                owner_version(&authority, &hash),
                WorkPermit::ResolveOnly,
            )
            .expect("trusted proposal checks out"),
    );
    let proposal = ProposalId(transaction.proposal_short_id());
    let facts = ChainTransitionFacts::for_foundation(
        next_view(85),
        block_changes(Vec::new(), Vec::new()),
        vec![proposal.clone()],
    )
    .expect("proposal transition facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("read-only validation does not cancel active work")
        .validate_for_foundation(vec![(proposal, ProposalWindowPosition::Outside)])
        .expect("the final proposal position is exhaustive");
    apply_plan(
        authority
            .plan_chain_transition(receipt)
            .expect("proposal expiry removes the active owner atomically"),
    );
    assert!(authority.entry(&hash).is_none());
    let stale = authority
        .apply_settlement(work.internal_failure())
        .expect_err("the old proposal capability is stale after terminalization");
    assert_eq!(
        stale.recovery(),
        &ComputeSettlementRecovery::Obsolete(StalePlan::Missing)
    );
    drop(stale);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_chain_recovery_receipt_proves_targeted_vacancy() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = output_transaction(545);
    let facts = ChainTransitionFacts::for_foundation(
        next_view(74),
        block_changes(Vec::new(), vec![transaction.clone()]),
        Vec::new(),
    )
    .expect("detached facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("vacancy is captured with the bounded recovery target")
        .validate_for_foundation(Vec::new())
        .expect("recovery requires no proposal facts");

    let admission = ValidatedAdmission::remote(transaction, ckb_network::PeerIndex::from(545))
        .expect("a concurrent targeted admission is structurally valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("the targeted hash becomes owned before chain Plan"),
    );
    let before = authority.normalized_snapshot();
    assert_eq!(
        authority.plan_chain_transition(receipt).err(),
        Some(PlanError::Stale(StalePlan::Version))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if entry.source.ingress_peer() == Some(ckb_network::PeerIndex::from(545))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_over_bound_causal_union_requires_generation_replacement_without_a_prefix() {
    let mut authority = TxPoolAuthority::for_foundation(large_chain_limits());
    let mut inputs = Vec::new();
    inputs
        .try_reserve(crate::constants::MAX_POOL_MUTATION_CANDIDATES + 1)
        .expect("fixture vector is small");
    for offset in 0..=crate::constants::MAX_POOL_MUTATION_CANDIDATES {
        let input = OutPoint::new(Byte32::new([(offset + 1) as u8; 32]), 0);
        let transaction = TransactionBuilder::default()
            .version(800 + offset as u32)
            .input(CellInput::new(input.clone(), 0))
            .build();
        let payload = resolved_payload_with_facts(
            &transaction,
            Vec::new(),
            vec![input.clone()],
            Capacity::shannons(10),
        );
        accept_remote_transaction_with_payload(
            &mut authority,
            transaction,
            800 + offset,
            AcceptedStatus::Pending,
            payload,
        );
        inputs.push(input);
    }
    let attached = inputs.into_iter().fold(
        TransactionBuilder::default().version(999u32),
        |builder, input| builder.input(CellInput::new(input, 0)),
    );
    let next = next_view(61);
    let facts = ChainTransitionFacts::for_foundation(
        next.clone(),
        block_changes(vec![attached.build()], Vec::new()),
        Vec::new(),
    )
    .expect("large hostile block facts remain canonical");
    let before = authority.normalized_snapshot();
    assert_eq!(
        authority.chain_validation_work(facts).err(),
        Some(PlanError::Backpressure(Backpressure::GenerationReplacement))
    );
    assert_eq!(authority.normalized_snapshot(), before);

    let committed = authority
        .plan_chain_generation_replacement(next, Vec::new())
        .expect("over-bound causal work converges through an empty fresh recovery cohort")
        .apply();
    drop(committed);
    assert_eq!(authority.chain_revision(), ChainRevision(1));
    assert_eq!(authority.owner_count(), 0);
    assert_resource_reference(&authority);
}

#[test]
fn uak_chain_batch_removes_over_policy_limit_shared_dependency_fanout() {
    let mut authority = TxPoolAuthority::for_foundation(large_chain_limits());
    let shared_dependency = OutPoint::new(Byte32::new([249; 32]), 0);
    let count = crate::constants::MAX_POOL_MUTATION_CANDIDATES + 1;
    let mut attached = Vec::with_capacity(count);
    let mut hashes = Vec::with_capacity(count);

    for offset in 0..count {
        let input = OutPoint::new(Byte32::new([(offset + 1) as u8; 32]), 0);
        let transaction = TransactionBuilder::default()
            .version(1_200 + offset as u32)
            .input(CellInput::new(input.clone(), 0))
            .cell_dep(
                CellDep::new_builder()
                    .out_point(shared_dependency.clone())
                    .build(),
            )
            .output(CellOutput::default())
            .output_data(Bytes::new().pack())
            .build();
        hashes.push(accept_remote_transaction_with_payload(
            &mut authority,
            transaction.clone(),
            1_200 + offset,
            AcceptedStatus::Pending,
            resolved_payload_with_facts(
                &transaction,
                Vec::new(),
                vec![input],
                Capacity::shannons(10),
            ),
        ));
        attached.push(transaction);
    }

    let facts = ChainTransitionFacts::for_foundation(
        next_view(62),
        block_changes(attached, Vec::new()),
        Vec::new(),
    )
    .expect("the canonical chain batch may exceed a local policy fanout bound");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("shared dependency identity fanout is not a chain reconciliation bound")
        .validate_for_foundation(Vec::new())
        .expect("the committed cohort needs no proposal facts");
    apply_plan(
        authority
            .plan_chain_transition(receipt)
            .expect("projected occupancy is computed without materializing every owner"),
    );

    assert!(hashes.iter().all(|hash| authority.entry(hash).is_none()));
    assert_eq!(
        authority.dependency_consumers_for_foundation(&DependencyKey::Cell(shared_dependency)),
        None
    );
    assert!(authority.primary_projection_consistent());
}
