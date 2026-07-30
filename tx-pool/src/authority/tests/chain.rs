use super::super::{
    chain::{FinalAdmissionError, ValidationRulesId},
    plan::{PlanError, StalePlan, TxPoolAuthority},
    state::{
        AcceptedStatus, ChainRevision, ChainViewId, ComputedOutcome, OwnedTx, PreAcceptedPhase,
        QueuedWork, VerifyCapability, WaitCondition, WorkPermit,
    },
    work::CheckedOutWork,
};
use super::foundation::{
    admit_remote, apply_without_work, assert_resource_reference, independent_batch, limits,
    missing_keys, owner_version, resolved_payload, tx, verify_remote_transaction,
};
use ckb_types::packed::Byte32;

#[test]
fn uak_final_admission_refreshes_stale_verification_context() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let candidate = verify_remote_transaction(&mut authority, tx(66), 66, Vec::new());
    let current_view = ChainViewId::new(ChainRevision(1), Byte32::new([66; 32]));
    authority.force_chain_view(current_view.clone());
    let version = owner_version(&authority, &candidate);

    apply_without_work(
        authority
            .plan_accept_for_foundation(&candidate, version, AcceptedStatus::Pending)
            .expect("fresh final validation replaces the old verification context"),
    );
    assert!(matches!(
        authority.entry(&candidate),
        Some(OwnedTx::Accepted(entry)) if entry.proof.admission_view() == &current_view
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_final_admission_receipt_is_stale_after_chain_view_aba() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let candidate = verify_remote_transaction(&mut authority, tx(67), 67, Vec::new());
    let batch = independent_batch(&authority, std::slice::from_ref(&candidate));
    authority.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::new([67; 32])));
    authority.force_chain_view(ChainViewId::new(ChainRevision(2), Byte32::zero()));
    let before = authority.normalized_snapshot();

    assert_eq!(
        authority.plan_settlement_for_foundation(&batch).err(),
        Some(PlanError::Stale(StalePlan::ChainRevision))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert_resource_reference(&authority);
}

#[test]
fn uak_final_admission_rejects_a_changed_validation_ruleset() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let candidate = verify_remote_transaction(&mut authority, tx(68), 68, Vec::new());
    let version = owner_version(&authority, &candidate);
    let before = authority.normalized_snapshot();

    assert_eq!(
        authority
            .final_admission_work(&candidate, version)
            .expect("verified owner yields final-validation work")
            .validate_for_foundation(AcceptedStatus::Pending, ValidationRulesId(1)),
        Err(FinalAdmissionError::ScriptRulesChanged)
    );
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_chain_tip_not_revision_controls_negative_evidence_freshness() {
    let mut authority = TxPoolAuthority::for_foundation(limits());

    let same_tip = admit_remote(&mut authority, 69, 69);
    let version = owner_version(&authority, &same_tip);
    let checkout = authority
        .plan_checkout_for_foundation(&same_tip, version, WorkPermit::ResolveOnly)
        .expect("same-tip resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("resolve work exists")
    else {
        panic!("resolve-only permit returns resolve work");
    };
    let missing = resolve
        .missing(missing_keys())
        .expect("fixture missing evidence is bounded");
    authority.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::zero()));
    apply_without_work(
        authority
            .apply_settlement(missing)
            .expect("same-tip negative evidence remains current"),
    );
    assert!(matches!(
        authority.entry(&same_tip),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Waiting(WaitCondition::Missing(_)))
    ));

    let changed_tip = admit_remote(&mut authority, 70, 70);
    let version = owner_version(&authority, &changed_tip);
    let checkout = authority
        .plan_checkout_for_foundation(&changed_tip, version, WorkPermit::ResolveOnly)
        .expect("changed-tip resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("resolve work exists")
    else {
        panic!("resolve-only permit returns resolve work");
    };
    let missing = resolve
        .missing(missing_keys())
        .expect("fixture missing evidence is bounded");
    authority.force_chain_view(ChainViewId::new(ChainRevision(2), Byte32::new([70; 32])));
    apply_without_work(
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
fn uak_matching_completion_settles_and_refreshes_across_chain_view_change() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 15, 34);
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("work exists") else {
        panic!("resolve-only permit returns resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    let settlement = resolve
        .yield_verify(payload)
        .expect("fixture resolution fits the checked-out work");
    let current_view = ChainViewId::new(ChainRevision(1), Byte32::new([15; 32]));
    authority.force_chain_view(current_view.clone());
    apply_without_work(
        authority
            .apply_settlement(settlement)
            .expect("the matching completion releases its old-view lease"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
    ));

    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            version,
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("reusable content checks out under the current view")
        .apply();
    let CheckedOutWork::Verify(verify) = checkout.into_work().expect("verify work exists") else {
        panic!("verify-only permit returns verify work");
    };
    let accepted_resident_bytes = verify.transaction().data().total_size();
    apply_without_work(
        authority
            .apply_settlement(
                verify
                    .verified(accepted_resident_bytes, 0)
                    .expect("current-view context validation succeeds"),
            )
            .expect("the refreshed verification settles"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                &entry.phase,
                PreAcceptedPhase::Computed(ComputedOutcome::Verified(verified))
                    if verified.chain_view() == &current_view
            )
    ));
    assert_resource_reference(&authority);
}
