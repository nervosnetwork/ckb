use super::foundation::{
    accept_remote_transaction, limits, owner_version, resolved_payload_with_facts,
    verify_remote_transaction_with_payload, verify_remote_transaction_with_payload_under,
};
use crate::authority::{
    plan::{
        CandidateDispositionPlan, FinalAdmissionDispositionPlan, IndependentCandidate,
        IndependentCoupling, SettlementBatch, SettlementPlan, TxPoolAuthority,
    },
    state::{AcceptedStatus, ChainRevision, ChainViewId, OwnedTx, PreAcceptedPhase, QueuedWork},
    validation::{
        FinalAdmissionValidation, FinalAdmissionValidationError, FinalAdmissionValidationOutcome,
    },
};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_script::TxVerifyEnv;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    U256,
    core::{Capacity, TransactionBuilder},
    packed::{Byte32, CellInput, CellOutput, OutPoint},
    prelude::Pack,
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

fn authority_at(snapshot: &Snapshot) -> TxPoolAuthority {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    authority.force_chain_view(ChainViewId::new(ChainRevision(0), snapshot.tip_hash()));
    authority
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
fn uak_script_rule_change_requeues_exact_content_for_verification() {
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
        FinalAdmissionValidationOutcome::Reverify(_)
    ));
    let FinalAdmissionDispositionPlan::Reverify(plan) = authority
        .plan_final_admission(outcome)
        .expect("the sealed revalidation outcome plans one owner transition")
    else {
        panic!("stale script evidence must not be accepted or terminalized");
    };
    let committed = plan.apply();

    assert_eq!(committed.retired_len(), 0);
    assert_ne!(owner_version(&authority, &key), old_version);
    assert!(matches!(
        authority.entry(&key),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
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
        panic!("valid evidence cannot become rejection or revalidation");
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
    let SettlementPlan::CoupledComponent {
        reason,
        disposition,
    } = authority
        .plan_settlement_for_foundation(&batch)
        .expect("refreshed payload routes through the retirement-aware compiler")
    else {
        panic!("a refreshed payload cannot use inline independent retirement");
    };
    assert_eq!(reason, IndependentCoupling::LocationRefreshedPayload);
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
