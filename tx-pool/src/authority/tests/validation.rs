use super::foundation::{
    accept_remote_transaction, admit_remote, apply_plan, direct_verified_facts_for_view, limits,
    owner_version, resolved_payload_with_facts, verify_remote_transaction_with_payload,
    verify_remote_transaction_with_payload_under,
};
use crate::authority::{
    chain::DirectAdmissionWork,
    plan::{
        CommittedDelta, PlanError, PreparedSharedDirectAdmissionDisposition,
        ReadyHeadCommitOutcome, SharedDirectRejectionTerminalOutcome, StalePlan, TxPoolAuthority,
    },
    state::{
        AcceptedStatus, ChainRevision, ChainViewId, OwnedTx, PreAcceptedPhase, QueuedWork,
        ValidatedAdmission,
    },
    validation::{
        DirectAdmissionValidation, DirectAdmissionValidationOutcome, FinalAdmissionValidation,
        FinalAdmissionValidationError, FinalAdmissionValidationOutcome, chain_sensitivity,
        verification_environment,
    },
};
use crate::{
    component::entry::accepted_transaction_charge_bytes, util::check_tx_fee_with_min_fee_rate,
};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_network::PeerIndex;
use ckb_script::TxVerifyEnv;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::{MockMedianTime, MockStore};
use ckb_types::{
    U256,
    bytes::Bytes,
    core::{
        Capacity, DepType, EpochNumberWithFraction, FeeRate, HeaderView, TransactionBuilder,
        TransactionInfo, cell::ResolvedTransaction,
    },
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};
use ckb_verification::{TimeRelativeTransactionVerifier, cache::ScriptVerificationRules};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocationDependentMetrics {
    fee: u64,
    accepted_resident_bytes: usize,
}

fn genesis_snapshot() -> Arc<Snapshot> {
    snapshot_at_height(0)
}

fn snapshot_at_height(number: u64) -> Arc<Snapshot> {
    let consensus = Arc::new(ConsensusBuilder::default().build());
    let store = MockStore::default();
    let tip = consensus
        .genesis_block()
        .header()
        .as_advanced_builder()
        .number(number)
        .epoch(EpochNumberWithFraction::new(
            number / 1_000,
            number % 1_000,
            1_000,
        ))
        .build();
    Arc::new(Snapshot::new(
        tip,
        U256::zero(),
        consensus.genesis_epoch_ext().clone(),
        store.store().get_snapshot(),
        Default::default(),
        consensus,
    ))
}

fn sensitivity_fixture(since: u64) -> ResolvedTransaction {
    let input = OutPoint::new(Byte32::new([80; 32]), 0);
    let code = OutPoint::new(Byte32::new([81; 32]), 0);
    let group = OutPoint::new(Byte32::new([82; 32]), 0);
    ResolvedTransaction::dummy_resolve(
        TransactionBuilder::default()
            .input(CellInput::new(input, since))
            .cell_dep(CellDep::new_builder().out_point(code).build())
            .cell_dep(
                CellDep::new_builder()
                    .out_point(group)
                    .dep_type(DepType::DepGroup)
                    .build(),
            )
            .build(),
    )
}

fn sensitivity_info(block_number: u64, index: usize) -> TransactionInfo {
    TransactionInfo::new(
        block_number,
        EpochNumberWithFraction::new(1, 0, 1),
        Byte32::new([83; 32]),
        index,
    )
}

fn time_relative_verifier_accepts(
    resolved: Arc<ResolvedTransaction>,
    block_number: u64,
    epoch_number: u64,
    cellbase_maturity: EpochNumberWithFraction,
) -> bool {
    let data_loader = MockMedianTime::new(vec![0; 11]);
    let parent_hash = data_loader.get_last_block_hash();
    let consensus = Arc::new(
        ConsensusBuilder::default()
            .median_time_block_count(11)
            .cellbase_maturity(cellbase_maturity)
            .build(),
    );
    let header = HeaderView::new_advanced_builder()
        .number(block_number)
        .epoch(EpochNumberWithFraction::new(epoch_number, 0, 1))
        .parent_hash(parent_hash)
        .build();
    TimeRelativeTransactionVerifier::new(
        resolved,
        consensus,
        data_loader,
        Arc::new(TxVerifyEnv::new_commit(&header)),
    )
    .verify()
    .is_ok()
}

fn assert_sensitivity_matches_verifier_observation(
    resolved: ResolvedTransaction,
    first_context: (u64, u64),
    second_context: (u64, u64),
    cellbase_maturity: EpochNumberWithFraction,
) {
    let resolved = Arc::new(resolved);
    let first = time_relative_verifier_accepts(
        Arc::clone(&resolved),
        first_context.0,
        first_context.1,
        cellbase_maturity,
    );
    let second = time_relative_verifier_accepts(
        Arc::clone(&resolved),
        second_context.0,
        second_context.1,
        cellbase_maturity,
    );
    assert_eq!(
        chain_sensitivity(&resolved).requires_reorg_revalidation(),
        first != second,
        "the retained sensitivity bit must equal an observed change in the real time-relative verifier"
    );
}

#[test]
fn uak_chain_sensitivity_refines_the_consensus_verifier_read_set() {
    let no_cellbase_maturity = EpochNumberWithFraction::new(0, 0, 1);
    let two_epoch_maturity = EpochNumberWithFraction::new(2, 0, 1);

    let stable = sensitivity_fixture(0);
    assert_sensitivity_matches_verifier_observation(stable, (4, 2), (5, 3), no_cellbase_maturity);

    let since = sensitivity_fixture(5);
    assert_sensitivity_matches_verifier_observation(since, (4, 3), (5, 3), no_cellbase_maturity);

    let mut regular_input = sensitivity_fixture(0);
    regular_input
        .resolved_inputs
        .first_mut()
        .expect("the fixture has one input")
        .transaction_info = Some(sensitivity_info(1, 1));
    assert_sensitivity_matches_verifier_observation(
        regular_input,
        (5, 2),
        (5, 3),
        two_epoch_maturity,
    );

    let mut genesis_cellbase = sensitivity_fixture(0);
    genesis_cellbase
        .resolved_inputs
        .first_mut()
        .expect("the fixture has one input")
        .transaction_info = Some(sensitivity_info(0, 0));
    assert_sensitivity_matches_verifier_observation(
        genesis_cellbase,
        (5, 2),
        (5, 3),
        two_epoch_maturity,
    );

    let mut input_cellbase = sensitivity_fixture(0);
    input_cellbase
        .resolved_inputs
        .first_mut()
        .expect("the fixture has one input")
        .transaction_info = Some(sensitivity_info(1, 0));
    assert_sensitivity_matches_verifier_observation(
        input_cellbase,
        (5, 2),
        (5, 3),
        two_epoch_maturity,
    );

    // Direct code deps and members expanded from a dep group share the
    // `resolved_cell_deps` role consumed by `MaturityVerifier`.
    let mut expanded_cellbase = sensitivity_fixture(0);
    expanded_cellbase
        .resolved_cell_deps
        .first_mut()
        .expect("the fixture has one resolved code dependency")
        .transaction_info = Some(sensitivity_info(1, 0));
    assert_sensitivity_matches_verifier_observation(
        expanded_cellbase,
        (5, 2),
        (5, 3),
        two_epoch_maturity,
    );

    // The dep-group container is location evidence but is not read by the
    // consensus maturity verifier. Marking it contextual would only cause
    // unnecessary revalidation after a payload-neutral detach.
    let mut group_container = sensitivity_fixture(0);
    group_container
        .resolved_dep_groups
        .first_mut()
        .expect("the fixture has one dep-group container")
        .transaction_info = Some(sensitivity_info(1, 0));
    assert_sensitivity_matches_verifier_observation(
        group_container,
        (5, 2),
        (5, 3),
        two_epoch_maturity,
    );
}

#[test]
fn uak_verification_environment_obeys_phase_owned_commit_bounds() {
    for closest in 1..=4 {
        let consensus = Arc::new(
            ConsensusBuilder::default()
                .tx_proposal_window(ckb_chain_spec::consensus::ProposalWindow(
                    closest,
                    closest + 2,
                ))
                .build(),
        );
        for (status, tip) in [
            (AcceptedStatus::Pending, 41),
            (AcceptedStatus::Gap, 42),
            (AcceptedStatus::Proposed, 43),
        ] {
            let store = MockStore::default();
            let header = consensus
                .genesis_block()
                .header()
                .as_advanced_builder()
                .number(tip)
                .epoch(EpochNumberWithFraction::new(
                    tip / 1_000,
                    tip % 1_000,
                    1_000,
                ))
                .build();
            let snapshot = Snapshot::new(
                header,
                U256::zero(),
                consensus.genesis_epoch_ext().clone(),
                store.store().get_snapshot(),
                Default::default(),
                Arc::clone(&consensus),
            );
            let window = consensus.tx_proposal_window();
            let production = verification_environment(status, &snapshot).block_number(window);
            let expected = match status {
                AcceptedStatus::Pending => tip + 1 + closest,
                AcceptedStatus::Gap => tip + closest,
                AcceptedStatus::Proposed => tip + 1,
            };
            assert_eq!(production, expected);
        }
    }
}

#[test]
fn uak_changed_tip_revalidates_header_dependencies() {
    let snapshot = genesis_snapshot();
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let missing_header = Byte32::new([61; 32]);
    let transaction = TransactionBuilder::default()
        .version(61u32)
        .header_dep(missing_header.clone())
        .build();
    let payload =
        resolved_payload_with_facts(&transaction, Vec::new(), Vec::new(), Capacity::shannons(1));
    let key = verify_remote_transaction_with_payload(&mut authority, transaction, 61, payload);
    authority.force_chain_view(ChainViewId::new(ChainRevision(1), snapshot.tip_hash()));
    let work = authority
        .final_admission_work(&key, owner_version(&authority, &key))
        .expect("Ready content can be revalidated under the new chain view");
    let result =
        FinalAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work)
            .expect("the new authority view and snapshot agree")
            .validate()
            .expect("invalid header is a candidate outcome, not an authority fault");

    assert!(matches!(&result,
        FinalAdmissionValidationOutcome::Rejected(rejection)
            if matches!(rejection.reason().reject(),
                crate::error::Reject::Resolve(
                ckb_types::core::error::OutPointError::InvalidHeader(header)
            ) if header == &missing_header)
    ));
    let committed = apply_shared_ready_head(&authority, &key, result);
    assert_eq!(committed.retired_len(), 1);
    assert!(authority.entry(&key).is_none());
}

#[test]
fn uak_final_malformed_revalidation_revokes_the_complete_peer_cohort() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let peer = 62;
    let input = OutPoint::new(Byte32::new([62; 32]), 0);
    let transaction = TransactionBuilder::default()
        .input(CellInput::new(input.clone(), u64::MAX))
        .build();
    let payload =
        resolved_payload_with_facts(&transaction, Vec::new(), vec![input], Capacity::shannons(1));
    let key = verify_remote_transaction_with_payload(&mut authority, transaction, peer, payload);
    let cohort_member = admit_remote(&mut authority, 6_201, peer);
    authority.force_chain_view(ChainViewId::new(ChainRevision(1), snapshot.tip_hash()));
    let work = authority
        .final_admission_work(&key, owner_version(&authority, &key))
        .expect("the Ready owner issues final-validation work");
    let outcome =
        FinalAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work)
            .expect("the authority and snapshot share one view")
            .validate()
            .expect("invalid since is a transaction outcome, not an authority fault");
    let FinalAdmissionValidationOutcome::Rejected(rejection) = &outcome else {
        panic!("invalid since must be rejected during final revalidation: {outcome:?}");
    };
    assert!(rejection.reason().is_malformed());

    let committed = apply_shared_ready_head(&authority, &key, outcome);
    assert_eq!(committed.retired_len(), 2);
    assert!(authority.entry(&key).is_none());
    assert!(authority.entry(&cohort_member).is_none());
    assert!(authority.peer_is_banned_for_reference(PeerIndex::from(peer)));
    assert!(authority.pending_recent_reject(&key).is_some());
    assert!(authority.primary_projection_consistent());
}

fn authority_at(snapshot: &Snapshot) -> TxPoolAuthority {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    authority.force_chain_view(ChainViewId::new(ChainRevision(0), snapshot.tip_hash()));
    authority
}

fn apply_shared_ready_head(
    authority: &TxPoolAuthority,
    key: &crate::authority::state::RawTxHash,
    outcome: FinalAdmissionValidationOutcome,
) -> CommittedDelta {
    let reservation = authority.reserve_ready_exact_for_foundation(std::slice::from_ref(key));
    let (mut reservations, remainder) = reservation
        .try_split_prefix(1)
        .unwrap_or_else(|_| panic!("one exact Ready head splits into one slot"));
    assert!(remainder.is_none());
    let reservation = reservations.pop().expect("one exact Ready head slot");
    let prepared = authority
        .prepare_shared_ready_head_disposition(outcome)
        .expect("the validated head compiles through its exact shared route");
    let committed = match prepared.commit(reservation) {
        ReadyHeadCommitOutcome::Committed(committed) => committed,
        ReadyHeadCommitOutcome::Stale { .. } => {
            panic!("the unchanged foundation cut cannot stale the shared Ready head")
        }
        ReadyHeadCommitOutcome::Fault { fault, .. } => {
            panic!("the valid foundation head cannot fault: {fault:?}")
        }
    };
    let (committed, post_commit_fault) = committed.into_parts();
    assert_eq!(post_commit_fault, None);
    committed
}

#[test]
fn uak_direct_validation_rejection_ignores_an_unrelated_accepted_commit() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let input = OutPoint::new(Byte32::new([64; 32]), 0);
    let transaction = Arc::new(
        TransactionBuilder::default()
            .version(6_303u32)
            .input(CellInput::new(input, 0))
            .build(),
    );
    let verified = direct_verified_facts_for_view(
        &transaction,
        authority.chain_view().clone(),
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1),
    );
    let work = DirectAdmissionWork::new(Arc::clone(&transaction), verified)
        .expect("direct work binds the exact transaction identity");
    let outcome =
        DirectAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work)
            .expect("direct validation captures one coherent authority cut")
            .validate()
            .expect("missing chain provenance is a transaction outcome");
    let DirectAdmissionValidationOutcome::Rejected(rejection) = outcome else {
        panic!("the fixture must produce a sealed final-validation rejection")
    };

    accept_remote_transaction(
        &mut authority,
        TransactionBuilder::default().version(6_304u32).build(),
        64,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    assert!(matches!(
        authority
            .plan_shared_direct_validation_rejection(rejection)
            .expect("the final rejection reserves its sole journal record")
            .apply(),
        SharedDirectRejectionTerminalOutcome::Committed { .. }
    ));
}

#[test]
fn uak_direct_validation_rejection_stales_on_a_relevant_accepted_producer() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let parent = TransactionBuilder::default()
        .version(6_305u32)
        .output(CellOutput::new_builder().build())
        .output_data(Bytes::new().pack())
        .build();
    let transaction = Arc::new(
        TransactionBuilder::default()
            .version(6_306u32)
            .input(CellInput::new(OutPoint::new(parent.hash(), 0), 0))
            .build(),
    );
    let verified = direct_verified_facts_for_view(
        &transaction,
        authority.chain_view().clone(),
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1),
    );
    let work = DirectAdmissionWork::new(Arc::clone(&transaction), verified)
        .expect("direct work binds the exact transaction identity");
    let outcome =
        DirectAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work)
            .expect("direct validation captures one coherent authority cut")
            .validate()
            .expect("missing chain provenance is a transaction outcome");
    let DirectAdmissionValidationOutcome::Rejected(rejection) = outcome else {
        panic!("the fixture must produce a sealed final-validation rejection")
    };

    accept_remote_transaction(
        &mut authority,
        parent,
        65,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    assert!(matches!(
        authority.plan_shared_direct_validation_rejection(rejection),
        Err(PlanError::Stale(StalePlan::AcceptedObservation))
    ));
}

#[test]
fn uak_direct_final_validation_reissues_the_dependency_observation_cut() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let parent = TransactionBuilder::default()
        .output(CellOutput::new_builder().build())
        .build();
    let parent_output = OutPoint::new(parent.hash(), 0);
    let admission = ValidatedAdmission::remote(parent, PeerIndex::from(65))
        .expect("the dependency-loss fixture is a valid retained admission");
    let parent_key = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("the parent acquires one retained owner"),
    );
    let parent_version = owner_version(&authority, &parent_key);
    apply_plan(
        authority
            .plan_terminalize_for_foundation(&parent_key, parent_version)
            .expect("terminal parent removal publishes definitive dependency loss"),
    );

    let transaction = Arc::new(
        TransactionBuilder::default()
            .input(CellInput::new(parent_output.clone(), 0))
            .build(),
    );
    let old_verified = direct_verified_facts_for_view(
        &transaction,
        authority.chain_view().clone(),
        Vec::new(),
        vec![parent_output],
        Capacity::shannons(1),
    );
    assert!(matches!(
        authority.prepare_production_shared_direct_admission_for_foundation(
            Arc::clone(&transaction),
            old_verified.clone(),
            AcceptedStatus::Pending,
        ),
        Err(PlanError::Stale(StalePlan::Dependency))
    ));

    let work = DirectAdmissionWork::new(Arc::clone(&transaction), old_verified)
        .expect("direct work binds the exact transaction identity");
    let outcome =
        DirectAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work)
            .expect("final validation captures the post-loss authority cut")
            .validate()
            .expect("same-tip positive chain evidence remains valid");
    let DirectAdmissionValidationOutcome::Candidate(receipt) = outcome else {
        panic!("final validation must reissue a current candidate receipt")
    };
    assert_eq!(
        receipt.proof().dependency_cut(),
        authority.dependency_observation_cut()
    );
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = authority
        .prepare_shared_direct_admission(receipt)
        .expect("the refreshed cut is current at the membership boundary")
    else {
        panic!("the independent direct transaction must be accepted")
    };
}

#[test]
fn uak_final_validation_rejects_a_mixed_authority_snapshot_cut() {
    let snapshot = genesis_snapshot();
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = TransactionBuilder::default().version(70u32).build();
    let payload =
        resolved_payload_with_facts(&transaction, Vec::new(), Vec::new(), Capacity::shannons(1));
    let key = verify_remote_transaction_with_payload(&mut authority, transaction, 70, payload);
    let work = authority
        .final_admission_work(&key, owner_version(&authority, &key))
        .expect("the Ready owner issues work under its original view");

    assert!(matches!(
        FinalAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work,),
        Err(FinalAdmissionValidationError::StaleView)
    ));
}

#[test]
fn uak_script_rule_change_requeues_the_exact_owner_for_resolution() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let current_rules = ScriptVerificationRules::from_env(
        snapshot.consensus(),
        &TxVerifyEnv::new_submit(snapshot.tip_header()),
    );
    let old_rules = match current_rules {
        ScriptVerificationRules::V0 => ScriptVerificationRules::V1,
        ScriptVerificationRules::V1 | ScriptVerificationRules::V2 => ScriptVerificationRules::V0,
    };
    let transaction = TransactionBuilder::default().version(71u32).build();
    let payload =
        resolved_payload_with_facts(&transaction, Vec::new(), Vec::new(), Capacity::shannons(1));
    let key = verify_remote_transaction_with_payload_under(
        &mut authority,
        transaction,
        71,
        payload,
        old_rules,
    );
    let old_version = owner_version(&authority, &key);
    let work = authority
        .final_admission_work(&key, old_version)
        .expect("the stale-rules Ready owner issues validation work");
    let outcome =
        FinalAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work)
            .expect("the authority and snapshot share one view")
            .validate()
            .expect("a hard-fork rules change is a normal candidate outcome");
    assert!(matches!(
        &outcome,
        FinalAdmissionValidationOutcome::Reresolve(_)
    ));
    let committed = apply_shared_ready_head(&authority, &key, outcome);

    assert_eq!(committed.retired_len(), 1);
    assert_ne!(owner_version(&authority, &key), old_version);
    assert!(matches!(
        authority.entry(&key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
}

#[test]
fn uak_final_validation_reuses_same_tip_positive_location_evidence() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let input = OutPoint::new([41; 32].pack(), 0);
    let transaction = TransactionBuilder::default()
        .input(CellInput::new(input.clone(), 0))
        .build();
    let payload =
        resolved_payload_with_facts(&transaction, Vec::new(), vec![input], Capacity::shannons(1));
    let key = verify_remote_transaction_with_payload(&mut authority, transaction, 41, payload);
    let version = owner_version(&authority, &key);
    let work = authority
        .final_admission_work(&key, version)
        .expect("the Ready owner issues one final-validation capability");
    let validation =
        FinalAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work)
            .expect("snapshot and authority expose one coherent view");
    let outcome = validation
        .validate()
        .expect("same-tip positive evidence needs no chain-cell lookup");
    let FinalAdmissionValidationOutcome::Candidate(receipt) = outcome else {
        panic!("same-tip valid evidence must reach membership planning");
    };

    assert!(
        receipt
            .proof()
            .is_chain_input(&OutPoint::new([41; 32].pack(), 0))
    );
    let committed = apply_shared_ready_head(
        &authority,
        &key,
        FinalAdmissionValidationOutcome::Candidate(receipt),
    );
    assert_eq!(committed.retired_len(), 1);
}

#[test]
fn uak_same_tip_unproven_location_is_rejected_not_treated_as_pool_origin() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let input = OutPoint::new([42; 32].pack(), 0);
    let transaction = TransactionBuilder::default()
        .input(CellInput::new(input.clone(), 0))
        .build();
    // `transaction_info == None` alone is not pool-origin evidence. No
    // Accepted producer exists in the paired authority overlay.
    let payload =
        resolved_payload_with_facts(&transaction, Vec::new(), Vec::new(), Capacity::shannons(1));
    let key = verify_remote_transaction_with_payload(&mut authority, transaction, 42, payload);
    let work = authority
        .final_admission_work(&key, owner_version(&authority, &key))
        .expect("the Ready owner issues final-validation work");
    let outcome =
        FinalAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work)
            .expect("the authority and snapshot share one view")
            .validate()
            .expect("missing provenance is a transaction outcome, not an authority fault");

    assert!(matches!(
        &outcome,
        FinalAdmissionValidationOutcome::Rejected(rejection)
            if matches!(
                rejection.reason().reject(),
                crate::error::Reject::Resolve(
                    ckb_types::core::error::OutPointError::Unknown(out_point)
                ) if out_point == &input
            )
    ));
    let committed = apply_shared_ready_head(&authority, &key, outcome);
    assert_eq!(committed.retired_len(), 1);
    assert!(authority.entry(&key).is_none());
}

#[test]
fn uak_pool_origin_refresh_is_coupled_and_retires_the_old_payload_outside_apply() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let parent_tx = TransactionBuilder::default()
        .version(51u32)
        .output(CellOutput::default())
        .build();
    let parent = accept_remote_transaction(
        &mut authority,
        parent_tx.clone(),
        51,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let input = OutPoint::new(parent_tx.hash(), 0);
    let child_tx = TransactionBuilder::default()
        .version(52u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let child_payload = resolved_payload_with_facts(
        &child_tx,
        Vec::new(),
        vec![input.clone()],
        Capacity::shannons(1),
    );
    let child = verify_remote_transaction_with_payload(&mut authority, child_tx, 52, child_payload);
    let version = owner_version(&authority, &child);
    let work = authority
        .final_admission_work(&child, version)
        .expect("the child is Ready");
    let retained_metrics = match authority.entry(&child) {
        Some(OwnedTx::PreAccepted(owner)) => match &owner.phase {
            PreAcceptedPhase::Ready(verified) => LocationDependentMetrics {
                fee: verified.metrics().fee.as_u64(),
                accepted_resident_bytes: verified.metrics().cost.resident_bytes,
            },
            _ => panic!("the child must remain Ready while validation owns its work"),
        },
        _ => panic!("the Ready child must remain a PreAccepted owner"),
    };
    assert!(
        work.payload()
            .resolved_transaction()
            .resolved_inputs
            .first()
            .expect("the child has one input")
            .transaction_info
            .is_some()
    );
    let outcome =
        FinalAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work)
            .expect("the Accepted parent is captured in the bounded overlay")
            .validate()
            .expect("the pool-produced input remains live");
    let FinalAdmissionValidationOutcome::Candidate(receipt) = outcome else {
        panic!("the live child must reach membership planning");
    };
    let recomputed_metrics = LocationDependentMetrics {
        fee: check_tx_fee_with_min_fee_rate(
            &snapshot,
            receipt.proof().payload().resolved_transaction(),
            receipt.proof().payload().serialized_bytes(),
            FeeRate::zero(),
        )
        .expect("the refreshed fixture satisfies the configured fee floor")
        .as_u64(),
        accepted_resident_bytes: accepted_transaction_charge_bytes(
            receipt.proof().payload().serialized_bytes(),
            receipt.proof().payload().resolved_transaction(),
        ),
    };
    let committed_metrics = LocationDependentMetrics {
        fee: receipt.proof().metrics().fee.as_u64(),
        accepted_resident_bytes: receipt.proof().metrics().cost.resident_bytes,
    };
    assert_eq!(committed_metrics, recomputed_metrics);
    assert_ne!(
        retained_metrics, recomputed_metrics,
        "the fixture must distinguish the old and refreshed location-dependent receipt"
    );
    assert!(!receipt.proof().is_chain_input(&input));
    let committed = apply_shared_ready_head(
        &authority,
        &child,
        FinalAdmissionValidationOutcome::Candidate(receipt),
    );
    assert_eq!(committed.retired_len(), 1);
    let Some(OwnedTx::Accepted(child_owner)) = authority.entry(&child) else {
        panic!("the admitted child must own the refreshed Accepted proof");
    };
    let accepted_input = child_owner
        .proof
        .payload()
        .resolved_transaction()
        .resolved_inputs
        .first()
        .expect("the accepted child has one input");
    assert!(accepted_input.transaction_info.is_none());

    let template = authority
        .read_view()
        .capture_template()
        .expect("the Accepted proof has one coherent template projection")
        .into_selection()
        .expect("template selection consumes the captured projection");
    let template_child = template
        .candidates()
        .iter()
        .find(|candidate| candidate.hash() == &child)
        .expect("the accepted child is externally visible to block construction");
    assert!(
        template_child
            .resolved()
            .resolved_inputs
            .first()
            .expect("the template child has one input")
            .transaction_info
            .is_none()
    );
    let parents = authority
        .accepted_parents(&child)
        .expect("the accepted child records its causal parent");
    assert_eq!(parents.len(), 1);
    assert!(parents.contains(&parent));
}

#[test]
fn uak_location_refresh_rechecks_the_configured_minimum_fee() {
    let snapshot = genesis_snapshot();
    let mut authority = authority_at(&snapshot);
    let parent_tx = TransactionBuilder::default()
        .version(53u32)
        .output(CellOutput::default())
        .build();
    accept_remote_transaction(
        &mut authority,
        parent_tx.clone(),
        53,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let input = OutPoint::new(parent_tx.hash(), 0);
    let child_tx = TransactionBuilder::default()
        .version(54u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let child_payload =
        resolved_payload_with_facts(&child_tx, Vec::new(), vec![input], Capacity::shannons(1));
    let child = verify_remote_transaction_with_payload(&mut authority, child_tx, 54, child_payload);
    let work = authority
        .final_admission_work(&child, owner_version(&authority, &child))
        .expect("the child is Ready");
    let outcome = FinalAdmissionValidation::capture_with_min_fee_rate_for_foundation(
        &authority,
        Arc::clone(&snapshot),
        work,
        FeeRate::from_u64(1_000),
    )
    .expect("the Accepted parent is captured")
    .validate()
    .expect("low fee after refresh is a transaction outcome");

    assert!(matches!(
        &outcome,
        FinalAdmissionValidationOutcome::Rejected(rejection)
            if matches!(rejection.reason().reject(), crate::error::Reject::LowFeeRate(..))
    ));
    let committed = apply_shared_ready_head(&authority, &child, outcome);
    assert_eq!(committed.retired_len(), 1);
    assert!(authority.entry(&child).is_none());
}
