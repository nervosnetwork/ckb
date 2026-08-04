use super::foundation::{
    accept_remote_transaction, admit_remote, apply_plan, direct_verified_facts_for_view, limits,
    owner_version, resolved_payload_with_facts, verify_remote_transaction_with_payload,
    verify_remote_transaction_with_payload_under,
};
use crate::authority::{
    chain::DirectAdmissionWork,
    plan::{
        CandidateDispositionPlan, DirectAdmissionDisposition, FinalAdmissionDispositionPlan,
        IndependentCandidate, PlanError, SettlementBatch, SettlementPlan, StalePlan,
        TxPoolAuthority,
    },
    state::{
        AcceptedStatus, ChainRevision, ChainViewId, OwnedTx, PreAcceptedPhase, QueuedWork,
        ValidatedAdmission,
    },
    validation::{
        DirectAdmissionValidation, DirectAdmissionValidationOutcome, FinalAdmissionValidation,
        FinalAdmissionValidationError, FinalAdmissionValidationOutcome,
    },
};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_network::PeerIndex;
use ckb_script::TxVerifyEnv;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    U256,
    core::{Capacity, TransactionBuilder},
    packed::{Byte32, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};
use ckb_verification::cache::ScriptVerificationRules;
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
    let FinalAdmissionDispositionPlan::ValidationRejected(rejection) = authority
        .plan_final_admission(result)
        .expect("the chain-bound rejection owns terminalization and publication")
    else {
        panic!("an invalid header must be a committed validation rejection");
    };
    assert!(matches!(
        rejection.reason().reject(),
        crate::error::Reject::Resolve(
            ckb_types::core::error::OutPointError::InvalidHeader(header)
        ) if header == &missing_header
    ));
    let (reason, committed) = rejection.apply();
    assert!(matches!(
        reason.reject(),
        crate::error::Reject::Resolve(
            ckb_types::core::error::OutPointError::InvalidHeader(header)
        ) if header == &missing_header
    ));
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

    let FinalAdmissionDispositionPlan::ValidationRejected(rejection) = authority
        .plan_final_admission(outcome)
        .expect("malformed final validation owns one peer-revocation disposition")
    else {
        panic!("malformed final validation cannot reach membership");
    };
    let (_, committed) = rejection.apply();
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

#[test]
fn uak_direct_validation_shares_the_final_validator_without_mutation_authority() {
    let snapshot = genesis_snapshot();
    let authority = authority_at(&snapshot);
    let transaction = Arc::new(TransactionBuilder::default().version(6_301u32).build());
    let verified = direct_verified_facts_for_view(
        &transaction,
        authority.chain_view().clone(),
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1),
    );
    let work = DirectAdmissionWork::new(
        Arc::clone(&transaction),
        authority.chain_view().clone(),
        verified,
    )
    .expect("direct work binds the exact transaction identity");
    let before = authority.normalized_snapshot();
    let outcome =
        DirectAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work)
            .expect("direct validation captures one coherent authority cut")
            .validate()
            .expect("the immutable direct candidate validates");

    let DirectAdmissionValidationOutcome::Candidate(receipt) = outcome else {
        panic!("the valid direct candidate must produce a membership receipt");
    };
    assert_eq!(
        receipt.transaction().witness_hash(),
        transaction.witness_hash()
    );
    assert_eq!(receipt.view(), authority.chain_view());
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_direct_validation_returns_a_sealed_rejection_without_mutation() {
    let snapshot = genesis_snapshot();
    let authority = authority_at(&snapshot);
    let input = OutPoint::new(Byte32::new([63; 32]), 0);
    let transaction = Arc::new(
        TransactionBuilder::default()
            .version(6_302u32)
            .input(CellInput::new(input.clone(), 0))
            .build(),
    );
    let verified = direct_verified_facts_for_view(
        &transaction,
        authority.chain_view().clone(),
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1),
    );
    let work = DirectAdmissionWork::new(
        Arc::clone(&transaction),
        authority.chain_view().clone(),
        verified,
    )
    .expect("direct work binds the exact transaction identity");
    let before = authority.normalized_snapshot();
    let outcome =
        DirectAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work)
            .expect("direct validation captures one coherent authority cut")
            .validate()
            .expect("missing chain provenance is a transaction outcome");

    assert!(matches!(
        &outcome,
        DirectAdmissionValidationOutcome::Rejected(rejection)
            if matches!(
                rejection.reason().reject(),
                crate::error::Reject::Resolve(
                    ckb_types::core::error::OutPointError::Unknown(out_point)
                ) if out_point == &input
            )
    ));
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_direct_validation_rejection_stales_with_its_accepted_source_cut() {
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
    let work = DirectAdmissionWork::new(
        Arc::clone(&transaction),
        authority.chain_view().clone(),
        verified,
    )
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
        authority.plan_direct_validation_rejection(rejection),
        Err(PlanError::Stale(StalePlan::SourceVersion))
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
        authority.plan_direct_admission_for_foundation(
            Arc::clone(&transaction),
            old_verified.clone(),
            AcceptedStatus::Pending,
        ),
        Err(PlanError::Stale(StalePlan::Dependency))
    ));

    let work = DirectAdmissionWork::new(
        Arc::clone(&transaction),
        authority.chain_view().clone(),
        old_verified,
    )
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
    let DirectAdmissionDisposition::Accepted(plan) = authority
        .plan_direct_admission(receipt)
        .expect("the refreshed cut is current at the membership boundary")
    else {
        panic!("the independent direct transaction must be accepted")
    };
    drop(plan);
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
    let FinalAdmissionDispositionPlan::Reresolve(plan) = authority
        .plan_final_admission(outcome)
        .expect("the sealed rules transition plans one owner transition")
    else {
        panic!("stale script evidence must return to resolution");
    };
    let committed = plan.apply();

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

    assert_eq!(
        receipt.payload_relation(),
        crate::authority::chain::ReadyPayloadRelation::Shared
    );
    assert!(
        receipt
            .proof()
            .is_chain_input(&OutPoint::new([41; 32].pack(), 0))
    );
    let outcome = FinalAdmissionValidationOutcome::Candidate(receipt);
    let FinalAdmissionDispositionPlan::Candidate(disposition) = authority
        .plan_final_admission(outcome)
        .expect("valid evidence reaches the one membership compiler")
    else {
        panic!("valid evidence cannot become rejection or re-resolution");
    };
    let CandidateDispositionPlan::Accepted(plan) = disposition else {
        panic!("an independent valid candidate must be accepted");
    };
    let committed = plan.apply();
    assert_eq!(committed.retired_len(), 0);
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
    let FinalAdmissionDispositionPlan::ValidationRejected(plan) = authority
        .plan_final_admission(outcome)
        .expect("the unproven location owns one terminal disposition")
    else {
        panic!("unproven same-tip metadata must not reach membership");
    };
    let (_, committed) = plan.apply();
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
    let outcome =
        FinalAdmissionValidation::capture_for_foundation(&authority, Arc::clone(&snapshot), work)
            .expect("the Accepted parent is captured in the bounded overlay")
            .validate()
            .expect("the pool-produced input remains live");
    let FinalAdmissionValidationOutcome::Candidate(receipt) = outcome else {
        panic!("the live child must reach membership planning");
    };

    assert_eq!(
        receipt.payload_relation(),
        crate::authority::chain::ReadyPayloadRelation::LocationRefreshed
    );
    assert!(!receipt.proof().is_chain_input(&input));
    let batch = SettlementBatch::new(vec![IndependentCandidate::new(receipt)])
        .expect("one Ready candidate is a valid batch");
    let SettlementPlan::CoupledComponent(disposition) = authority
        .plan_settlement(&batch)
        .expect("refreshed payload routes through the retirement-aware compiler")
    else {
        panic!("a refreshed payload cannot use inline independent retirement");
    };
    let CandidateDispositionPlan::Accepted(plan) = disposition else {
        panic!("a live child of an Accepted parent must be admitted");
    };
    let committed = plan.apply();
    assert_eq!(committed.retired_len(), 1);
    let parents = authority
        .accepted_parents(&child)
        .expect("the accepted child records its causal parent");
    assert_eq!(parents.len(), 1);
    assert!(parents.contains(&parent));
}
