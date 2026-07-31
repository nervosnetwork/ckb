use super::super::{
    chain::{
        ChainBlockChanges, ChainPackagingMode, ChainTransitionFacts, FinalAdmissionError,
        ProposalWindowPosition,
    },
    effect::{CommittedEffect, CommittedRejection},
    plan::{Backpressure, PlanError, StalePlan, TxPoolAuthority},
    resources::{AcceptedResources, ComputeLimits, ResourceLimits, ResourceVector},
    state::{
        AcceptedStatus, ChainRevision, ChainViewId, DependencyKey, OwnedTx, PreAcceptedPhase,
        PreAcceptedSource, ProposalBase, ProposalContextId, ProposalId, QueuedWork, RawTxHash,
        RemoteDeadline, RemotePayloadOrigin, TxIdentity, ValidatedAdmission, VerifyCapability,
        WorkPermit,
    },
    work::CheckedOutWork,
};
use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, admit_remote,
    admit_remote_until, apply_without_work, assert_membership_reference, assert_resource_reference,
    independent_batch, limits, missing_keys, owner_version, resolved_payload,
    resolved_payload_with_facts, tx, verify_remote_transaction,
    verify_remote_transaction_with_payload,
};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, FeeRate, TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};
use ckb_verification::cache::ScriptVerificationRules;
use std::{collections::HashSet, num::NonZeroUsize};

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
            .validate_for_foundation(AcceptedStatus::Pending, ScriptVerificationRules::V1),
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
            if matches!(entry.phase, PreAcceptedPhase::Waiting(_))
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
                PreAcceptedPhase::Ready(verified)
                    if verified.chain_view() == &current_view
            )
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_chain_change_requeues_chain_bound_results_but_commits_resource_rejections() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let mut settlements = Vec::new();
    for (nonce, peer) in [(71, 71), (72, 72), (73, 73)] {
        let hash = admit_remote(&mut authority, nonce, peer);
        let (_, work) = super::foundation::take_resolve_work(
            authority
                .plan_checkout_for_foundation(
                    &hash,
                    owner_version(&authority, &hash),
                    WorkPermit::ResolveOnly,
                )
                .expect("fixture resolve checkout plans")
                .apply(),
        );
        settlements.push((hash, work));
    }
    authority.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::new([71; 32])));

    let (rejected_hash, rejected_work) = settlements.remove(0);
    apply_without_work(
        authority
            .apply_settlement(rejected_work.rejected(super::super::state::RejectionKind::Policy))
            .expect("a stale contextual rejection releases its exact lease"),
    );
    assert!(matches!(
        authority.entry(&rejected_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));

    let (budget_hash, budget_work) = settlements.remove(0);
    let budget = budget_work
        .missing(vec![DependencyKey::Header(Byte32::zero()); 17])
        .expect("over-grant missing evidence becomes a typed resource rejection");
    apply_without_work(
        authority
            .apply_settlement(budget)
            .expect("budget failure is independent of chain context"),
    );
    assert!(authority.entry(&budget_hash).is_none());

    let (internal_hash, internal_work) = settlements.remove(0);
    apply_without_work(
        authority
            .apply_settlement(internal_work.internal_failure())
            .expect("internal worker failure releases and retries its lease"),
    );
    assert!(matches!(
        authority.entry(&internal_hash),
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
        .plan_checkout_for_foundation(
            hash,
            owner_version(authority, hash),
            WorkPermit::ResolveOnly,
        )
        .expect("missing fixture checks out")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("resolve work exists")
    else {
        panic!("resolve-only permit returns resolve work");
    };
    let settlement = resolve
        .missing(vec![dependency])
        .expect("missing fixture is bounded");
    apply_without_work(
        authority
            .apply_settlement(settlement)
            .expect("missing fixture enters Wait"),
    );
}

fn drain_dependency_maintenance(authority: &mut TxPoolAuthority) {
    while let Some(plan) = authority
        .plan_dependency_maintenance_for_foundation()
        .expect("dependency maintenance remains coherent")
    {
        apply_without_work(plan);
    }
}

#[test]
fn uak_replacement_history_survives_winner_commit_and_wakes_after_reorg() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let chain_input = OutPoint::new(Byte32::new([67; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(541u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        541,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![chain_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let winner_tx = TransactionBuilder::default()
        .version(542u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let winner = verify_remote_transaction_with_payload(
        &mut authority,
        winner_tx.clone(),
        542,
        resolved_payload_with_facts(
            &winner_tx,
            Vec::new(),
            vec![chain_input],
            Capacity::shannons(10_000),
        ),
    );
    let winner_version = owner_version(&authority, &winner);
    apply_without_work(
        authority
            .plan_accept_for_foundation(&winner, winner_version, AcceptedStatus::Pending)
            .expect("the funded winner retains its accepted victim"),
    );
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::ReplacementHistory(_))
    ));

    let committed = ChainTransitionFacts::for_foundation(
        next_view(67),
        block_changes(vec![winner_tx.clone()], Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("winner commit facts are canonical");
    let committed = authority
        .chain_validation_work(committed)
        .expect("winner commit preserves parked history")
        .validate_for_foundation(Vec::new())
        .expect("winner commit needs no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(committed)
            .expect("winner commit and history preservation are one Apply"),
    );
    assert!(authority.entry(&winner).is_none());
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::ReplacementHistory(_))
    ));

    let detached = ChainTransitionFacts::for_foundation(
        ChainViewId::new(ChainRevision(2), Byte32::new([68; 32])),
        block_changes(Vec::new(), vec![winner_tx]),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("winner detach facts are canonical");
    let detached = authority
        .chain_validation_work(detached)
        .expect("winner detach publishes its chain input availability")
        .validate_for_foundation(Vec::new())
        .expect("winner detach needs no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(detached)
            .expect("newer availability coalesces behind the undrained loss"),
    );
    drain_dependency_maintenance(&mut authority);

    assert!(matches!(
        authority.entry(&winner),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_eq!(authority.resources().replacement_history().entries, 0);
    assert_resource_reference(&authority);
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

fn empty_transition(
    authority: &mut TxPoolAuthority,
    byte: u8,
) -> super::super::plan::CommittedDelta {
    let facts = ChainTransitionFacts::for_foundation(
        next_view(byte),
        block_changes(Vec::new(), Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("empty chain facts are canonical");
    let work = authority
        .chain_validation_work(facts)
        .expect("empty affected slice plans");
    let receipt = work
        .validate_for_foundation(Vec::new())
        .expect("empty slice has no proposal facts");
    authority
        .plan_chain_transition(receipt)
        .expect("empty chain transition applies the new view")
        .apply()
}

#[test]
fn uak_chain_commit_removes_a_parent_without_stranding_its_surviving_child() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(501);
    let parent = accept_remote_transaction(
        &mut authority,
        parent_tx.clone(),
        501,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let child_tx = child_transaction(502, &parent_tx);
    let child = accept_remote_transaction(
        &mut authority,
        child_tx,
        502,
        AcceptedStatus::Gap,
        Vec::new(),
    );

    let facts = ChainTransitionFacts::for_foundation(
        next_view(51),
        block_changes(vec![parent_tx], Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::Package,
    )
    .expect("attached facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("committed parent produces one bounded work slice")
        .validate_for_foundation(Vec::new())
        .expect("no status validation is required");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("committed-parent removal is one atomic chain Apply"),
    );

    assert!(authority.entry(&parent).is_none());
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(
        authority
            .accepted_parents(&child)
            .is_some_and(HashSet::is_empty)
    );
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_chain_commit_closes_a_preaccepted_owner_with_exact_effect_semantics() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = output_transaction(503);
    let admission =
        ValidatedAdmission::remote(transaction.clone(), ckb_network::PeerIndex::from(503))
            .expect("preaccepted committed fixture is valid");
    let hash = admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(admission)
            .expect("fixture enters preacceptance"),
    );
    let facts = ChainTransitionFacts::for_foundation(
        next_view(72),
        block_changes(vec![transaction.clone()], Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("attached preaccepted fact is canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("committed raw owner is selected")
        .validate_for_foundation(Vec::new())
        .expect("commit needs no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("owner removal and effect append are atomic"),
    );
    assert!(authority.entry(&hash).is_none());
    let lease = authority
        .plan_effect_checkout_for_foundation()
        .expect("effect checkout plans")
        .expect("chain commit queued one exact effect")
        .apply()
        .into_effect_lease()
        .expect("effect checkout returns its capability");
    assert_eq!(
        lease.effects(),
        &[CommittedEffect::ChainCommitted {
            tx: std::sync::Arc::new(transaction),
        }]
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_chain_conflict_subtracts_a_deep_closure_from_its_surviving_ancestor() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let ancestor_tx = output_transaction(511);
    let ancestor = accept_remote_transaction(
        &mut authority,
        ancestor_tx.clone(),
        511,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let chain_input = OutPoint::new(Byte32::new([52; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(512u32)
        .input(CellInput::new(OutPoint::new(ancestor_tx.hash(), 0), 0))
        .input(CellInput::new(chain_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        512,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![chain_input.clone()],
            Capacity::shannons(10),
        ),
    );
    let child_tx = child_transaction(513, &victim_tx);
    let child = accept_remote_transaction(
        &mut authority,
        child_tx,
        513,
        AcceptedStatus::Gap,
        Vec::new(),
    );
    let attached = TransactionBuilder::default()
        .version(514u32)
        .input(CellInput::new(chain_input, 0))
        .build();

    let facts = ChainTransitionFacts::for_foundation(
        next_view(52),
        block_changes(vec![attached], Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("conflict facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("one traversal captures the complete conflict closure")
        .validate_for_foundation(Vec::new())
        .expect("the closure has no status survivors");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("deep closure removal projects atomically"),
    );

    assert!(authority.entry(&victim).is_none());
    assert!(authority.entry(&child).is_none());
    assert_eq!(
        authority
            .membership_snapshot_for_reference()
            .descendant_aggregates[&ancestor]
            .entries,
        1
    );
    assert_membership_reference(&authority);
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
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("cell-dep conflict facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("canonical dependency closure includes accepted cell-dep readers")
        .validate_for_foundation(Vec::new())
        .expect("the closed conflict set needs no proposal facts");
    apply_without_work(
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
fn uak_conflict_dominates_simultaneous_recovery_for_accepted_and_preaccepted_owners() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let detached_parent = output_transaction(1_320);
    let parent_output = OutPoint::new(detached_parent.hash(), 0);
    let chain_input = OutPoint::new(Byte32::new([77; 32]), 0);
    let accepted_tx = TransactionBuilder::default()
        .version(1_321u32)
        .input(CellInput::new(parent_output.clone(), 0))
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let accepted = accept_remote_transaction_with_payload(
        &mut authority,
        accepted_tx.clone(),
        1_321,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &accepted_tx,
            Vec::new(),
            vec![parent_output.clone(), chain_input.clone()],
            Capacity::shannons(10),
        ),
    );
    let preaccepted_tx = TransactionBuilder::default()
        .version(1_322u32)
        .input(CellInput::new(parent_output, 0))
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let preaccepted_admission =
        ValidatedAdmission::remote(preaccepted_tx, ckb_network::PeerIndex::from(1_322))
            .expect("preaccepted dual-cause fixture is valid");
    let preaccepted = preaccepted_admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(preaccepted_admission)
            .expect("preaccepted dual-cause fixture enters ownership"),
    );
    let attached = TransactionBuilder::default()
        .version(1_323u32)
        .input(CellInput::new(chain_input, 0))
        .build();
    let recovered_parent = RawTxHash(detached_parent.hash());
    let facts = ChainTransitionFacts::for_foundation(
        next_view(77),
        block_changes(vec![attached], vec![detached_parent]),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("conflict and recovery facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("the closed causal lattice selects every affected owner")
        .validate_for_foundation(Vec::new())
        .expect("conflict-dominated owners need no proposal-window facts");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("conflict, recovery, and detached replay apply atomically"),
    );

    assert!(authority.entry(&accepted).is_none());
    assert!(authority.entry(&preaccepted).is_none());
    assert!(matches!(
        authority.entry(&recovered_parent),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_attached_conflict_terminalizes_preaccepted_without_trust_promotion() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let input = OutPoint::new(Byte32::new([62; 32]), 0);
    let candidate_tx = TransactionBuilder::default()
        .version(515u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let candidate_admission = super::super::state::ValidatedAdmission::remote(
        candidate_tx.clone(),
        ckb_network::PeerIndex::from(515),
    )
    .expect("remote conflict fixture is valid");
    let candidate = candidate_admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(candidate_admission)
            .expect("remote conflict fixture enters preacceptance"),
    );
    let attached = TransactionBuilder::default()
        .version(516u32)
        .input(CellInput::new(input, 0))
        .build();
    let facts = ChainTransitionFacts::for_foundation(
        next_view(62),
        block_changes(vec![attached], Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("attached conflict facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("preaccepted conflict is a bounded terminal disposition")
        .validate_for_foundation(Vec::new())
        .expect("terminal conflict needs no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("conflict removal and chain cut commit together"),
    );
    assert!(authority.entry(&candidate).is_none());

    let refetch = super::super::state::ValidatedAdmission::remote(
        candidate_tx,
        ckb_network::PeerIndex::from(516),
    )
    .expect("another peer may provide the same raw transaction later");
    apply_without_work(
        authority
            .plan_admission(refetch)
            .expect("chain conflict creates no raw-hash tombstone"),
    );
    assert!(matches!(
        authority.entry(&candidate),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_chain_conflict_commits_the_canonical_dead_outpoint() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let smaller = OutPoint::new(Byte32::new([64; 32]), 0);
    let larger = OutPoint::new(Byte32::new([65; 32]), 0);
    let candidate_tx = TransactionBuilder::default()
        .version(520u32)
        .input(CellInput::new(larger.clone(), 0))
        .input(CellInput::new(smaller.clone(), 0))
        .build();
    let admission =
        ValidatedAdmission::remote(candidate_tx.clone(), ckb_network::PeerIndex::from(520))
            .expect("multi-conflict fixture is valid");
    let candidate = admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(admission)
            .expect("multi-conflict fixture enters preacceptance"),
    );

    // Deliberately present the larger cell first. The causal join must not
    // leak traversal or hash-map order into the public rejection reason.
    let attached = TransactionBuilder::default()
        .version(521u32)
        .input(CellInput::new(larger, 0))
        .input(CellInput::new(smaller.clone(), 0))
        .build();
    let facts = ChainTransitionFacts::for_foundation(
        next_view(64),
        block_changes(vec![attached], Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("multi-conflict facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("one causal plan joins both conflict cells")
        .validate_for_foundation(Vec::new())
        .expect("terminal conflict needs no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("owner removal and exact conflict effect commit together"),
    );
    assert!(authority.entry(&candidate).is_none());

    let lease = authority
        .plan_effect_checkout_for_foundation()
        .expect("effect checkout plans")
        .expect("chain conflict publishes one exact rejection")
        .apply()
        .into_effect_lease()
        .expect("effect checkout returns its capability");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::Rejected {
            reason: CommittedRejection::ChainConflict { out_point },
            ..
        }] if out_point == &smaller
    ));
    apply_without_work(
        authority
            .apply_effect_settlement_for_foundation(lease.published())
            .expect("published conflict effect settles"),
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_chain_conflict_marks_a_removed_preaccepted_parents_active_child_stale() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let chain_input = OutPoint::new(Byte32::new([63; 32]), 0);
    let parent_tx = TransactionBuilder::default()
        .version(517u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent_admission =
        ValidatedAdmission::remote(parent_tx.clone(), ckb_network::PeerIndex::from(517))
            .expect("preaccepted parent fixture is valid");
    let parent = parent_admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(parent_admission)
            .expect("preaccepted parent enters ownership"),
    );

    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let child_tx = child_transaction(518, &parent_tx);
    let child_admission = ValidatedAdmission::remote(child_tx, ckb_network::PeerIndex::from(518))
        .expect("dependent child fixture is valid");
    let child = child_admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(child_admission)
            .expect("dependent child enters ownership"),
    );
    let (_, work) = super::foundation::take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                &child,
                owner_version(&authority, &child),
                WorkPermit::ResolveOnly,
            )
            .expect("dependent child checks out")
            .apply(),
    );

    let attached = TransactionBuilder::default()
        .version(519u32)
        .input(CellInput::new(chain_input, 0))
        .build();
    let facts = ChainTransitionFacts::for_foundation(
        next_view(63),
        block_changes(vec![attached], Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("attached conflict facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("the parent conflict preserves the active child capability")
        .validate_for_foundation(Vec::new())
        .expect("the conflict needs no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("parent removal and dependency loss commit atomically"),
    );

    assert!(authority.entry(&parent).is_none());
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    apply_without_work(
        authority
            .apply_settlement(
                work.missing(vec![DependencyKey::Cell(parent_output)])
                    .expect("the old dependency result is bounded"),
            )
            .expect("the exact lease settles after the dependency cut"),
    );
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
    assert_resource_reference(&authority);
}

#[test]
fn uak_chain_projection_combines_status_and_aggregate_changes_once() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let ancestor_tx = output_transaction(521);
    let ancestor = accept_remote_transaction(
        &mut authority,
        ancestor_tx.clone(),
        521,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let chain_input = OutPoint::new(Byte32::new([53; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(522u32)
        .input(CellInput::new(OutPoint::new(ancestor_tx.hash(), 0), 0))
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let victim_payload = resolved_payload_with_facts(
        &victim_tx,
        Vec::new(),
        vec![chain_input.clone()],
        Capacity::shannons(10),
    );
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx,
        522,
        AcceptedStatus::Pending,
        victim_payload,
    );
    let attached = TransactionBuilder::default()
        .version(523u32)
        .input(CellInput::new(chain_input, 0))
        .build();
    let proposal = TxIdentity::from_transaction(&ancestor_tx).proposal;
    let facts = ChainTransitionFacts::for_foundation(
        next_view(53),
        block_changes(vec![attached], Vec::new()),
        vec![proposal.clone()],
        Vec::new(),
        ChainPackagingMode::Package,
    )
    .expect("status and conflict facts are canonical");
    let work = authority
        .chain_validation_work(facts)
        .expect("status and aggregate share one affected slice");
    assert_eq!(
        work.required_proposals().expect("proposal list fits"),
        vec![proposal.clone()]
    );
    let receipt = work
        .validate_for_foundation(vec![(proposal, ProposalWindowPosition::Gap)])
        .expect("the final proposal position is exhaustive");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("status and aggregate keys change in one projection"),
    );

    assert!(authority.entry(&victim).is_none());
    assert!(matches!(
        authority.entry(&ancestor),
        Some(OwnedTx::Accepted(entry)) if entry.status() == AcceptedStatus::Gap
    ));
    assert_eq!(
        authority
            .membership_snapshot_for_reference()
            .descendant_aggregates[&ancestor]
            .entries,
        1
    );
    assert_membership_reference(&authority);
}

#[test]
fn uak_detached_parent_and_accepted_child_recover_parent_first() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let detached_parent = output_transaction(531);
    let parent_output = OutPoint::new(detached_parent.hash(), 0);
    let child_tx = child_transaction(532, &detached_parent);
    let child = accept_remote_transaction_with_payload(
        &mut authority,
        child_tx.clone(),
        532,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &child_tx,
            Vec::new(),
            vec![parent_output],
            Capacity::shannons(10),
        ),
    );
    let parent = RawTxHash(detached_parent.hash());
    let facts = ChainTransitionFacts::for_foundation(
        next_view(54),
        block_changes(Vec::new(), vec![detached_parent]),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("detached facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("detached origin finds its accepted descendant")
        .validate_for_foundation(Vec::new())
        .expect("recovery requires no proposal positions");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("parent and descendant move to recovery atomically"),
    );

    let parent_arrival = authority
        .entry(&parent)
        .expect("parent recovered")
        .record()
        .arrival;
    let child_arrival = authority
        .entry(&child)
        .expect("child recovered")
        .record()
        .arrival;
    assert!(parent_arrival < child_arrival);
    assert!(matches!(
        authority.entry(&parent),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_eq!(authority.membership_counts().pending, 0);
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
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("detached provider facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("canonical dependency closure includes the accepted cell-dep reader")
        .validate_for_foundation(Vec::new())
        .expect("provider recovery requires no proposal facts");
    apply_without_work(
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
fn uak_detached_header_requeues_its_accepted_consumer_in_the_same_apply() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let detached_header = Byte32::new([78; 32]);
    let consumer_tx = TransactionBuilder::default()
        .version(1_322u32)
        .header_dep(detached_header.clone())
        .build();
    let consumer = accept_remote_transaction(
        &mut authority,
        consumer_tx,
        1_322,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let facts = ChainTransitionFacts::for_foundation(
        next_view(79),
        ChainBlockChanges::for_foundation(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![detached_header],
        ),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("detached header facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("canonical dependency closure includes the accepted header consumer")
        .validate_for_foundation(Vec::new())
        .expect("header recovery requires no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("header loss and consumer recovery commit atomically"),
    );

    assert!(matches!(
        authority.entry(&consumer),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_eq!(authority.membership_counts().pending, 0);
    assert!(authority.primary_projection_consistent());
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_recovery_orders_cell_dependencies_before_hash_tiebreaks() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(1_100);
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let child_tx = (1_101u32..1_300)
        .map(|version| {
            TransactionBuilder::default()
                .version(version)
                .cell_dep(
                    CellDep::new_builder()
                        .out_point(parent_output.clone())
                        .build(),
                )
                .build()
        })
        .find(|transaction| transaction.hash() < parent_tx.hash())
        .expect("fixture finds a child hash that would sort before its parent");
    let parent = accept_remote_transaction(
        &mut authority,
        parent_tx.clone(),
        1_100,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let child = accept_remote_transaction(
        &mut authority,
        child_tx.clone(),
        1_101,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let facts = ChainTransitionFacts::for_foundation(
        next_view(68),
        block_changes(Vec::new(), vec![child_tx, parent_tx]),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("detached dependency facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("declared cell dependency participates in recovery ordering")
        .validate_for_foundation(Vec::new())
        .expect("recovery requires no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("dependency-ordered recovery installs atomically"),
    );
    assert!(
        authority
            .entry(&parent)
            .expect("parent recovered")
            .record()
            .arrival
            < authority
                .entry(&child)
                .expect("child recovered")
                .record()
                .arrival
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_relocated_chain_producer_requeues_accepted_consumers() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let relocated_parent = output_transaction(1_310);
    let parent_output = OutPoint::new(relocated_parent.hash(), 0);
    let child_tx = child_transaction(1_311, &relocated_parent);
    let child_payload = resolved_payload_with_facts(
        &child_tx,
        Vec::new(),
        vec![parent_output],
        Capacity::shannons(10),
    );
    let child = accept_remote_transaction_with_payload(
        &mut authority,
        child_tx,
        1_311,
        AcceptedStatus::Pending,
        child_payload,
    );
    let facts = ChainTransitionFacts::for_foundation(
        next_view(70),
        block_changes(vec![relocated_parent.clone()], vec![relocated_parent]),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("same-raw cross-fork facts preserve relocation provenance");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("relocated origin selects its accepted consumer")
        .validate_for_foundation(Vec::new())
        .expect("relocation recovery requires no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("old location/time proof is removed with the chain cut"),
    );
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_direct_recovery_dominates_a_simultaneous_proposal_status_change() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = output_transaction(1_314);
    let proposal = ProposalId(transaction.proposal_short_id());
    let hash = accept_remote_transaction(
        &mut authority,
        transaction.clone(),
        1_314,
        AcceptedStatus::Gap,
        Vec::new(),
    );
    let facts = ChainTransitionFacts::for_foundation(
        next_view(73),
        block_changes(Vec::new(), vec![transaction]),
        vec![proposal],
        Vec::new(),
        ChainPackagingMode::Package,
    )
    .expect("detached recovery and proposal facts are canonical");
    let work = authority
        .chain_validation_work(facts)
        .expect("final PreAccepted ownership suppresses an obsolete status change");
    assert!(
        work.required_proposals()
            .expect("the projected status set is bounded")
            .is_empty()
    );
    let receipt = work
        .validate_for_foundation(Vec::new())
        .expect("recovery requires no proposal-window result");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("one final owner change applies without duplication"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_causal_recovery_dominates_a_detached_proposal_demotion() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let detached_parent = output_transaction(1_324);
    let parent_output = OutPoint::new(detached_parent.hash(), 0);
    let child_tx = child_transaction(1_325, &detached_parent);
    let proposal = ProposalId(child_tx.proposal_short_id());
    let child = accept_remote_transaction_with_payload(
        &mut authority,
        child_tx.clone(),
        1_325,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &child_tx,
            Vec::new(),
            vec![parent_output],
            Capacity::shannons(10),
        ),
    );
    let facts = ChainTransitionFacts::for_foundation(
        next_view(78),
        block_changes(Vec::new(), vec![detached_parent]),
        Vec::new(),
        vec![proposal],
        ChainPackagingMode::Package,
    )
    .expect("causal recovery and detached proposal facts are canonical");
    let work = authority
        .chain_validation_work(facts)
        .expect("recovery is the stronger causal action");
    assert!(
        work.required_proposals()
            .expect("the bounded proposal set is materialized")
            .is_empty(),
        "a recovered owner must not retain a second status transition"
    );
    let receipt = work
        .validate_for_foundation(Vec::new())
        .expect("recovery requires no proposal-window fact");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("the stronger recovery action applies once"),
    );
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
    assert_resource_reference(&authority);
}

#[test]
fn uak_reorg_requeues_only_context_sensitive_accepted_membership() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let contextual_tx = output_transaction(1_312);
    let contextual = verify_remote_transaction(&mut authority, contextual_tx, 1_312, Vec::new());
    apply_without_work(
        authority
            .plan_accept_context_sensitive_for_foundation(
                &contextual,
                owner_version(&authority, &contextual),
                AcceptedStatus::Pending,
            )
            .expect("context-sensitive acceptance is sealed by validation"),
    );
    let stable = accept_remote_transaction(
        &mut authority,
        output_transaction(1_313),
        1_313,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let facts = ChainTransitionFacts::for_foundation(
        next_view(71),
        ChainBlockChanges::for_foundation(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![Byte32::new([71; 32])],
        ),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("blank-fork detach facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("the derived sensitivity index avoids a full owner scan")
        .validate_for_foundation(Vec::new())
        .expect("context recovery requires no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("context-sensitive recovery and chain cut are atomic"),
    );
    assert!(matches!(
        authority.entry(&contextual),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(matches!(
        authority.entry(&stable),
        Some(OwnedTx::Accepted(_))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_rules_transition_cannot_claim_monotonic_accepted_validity() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let contextual_tx = output_transaction(1_315);
    let contextual = verify_remote_transaction(&mut authority, contextual_tx, 1_315, Vec::new());
    apply_without_work(
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
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("an extension without block deltas is canonical")
    .revalidate_context_for_foundation();
    let receipt = authority
        .chain_validation_work(facts)
        .expect("typed context invalidation selects only its derived index")
        .validate_for_foundation(Vec::new())
        .expect("context recovery requires no proposal facts");
    apply_without_work(
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
        Some(OwnedTx::Accepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_chain_recovery_preserves_a_preaccepted_dependents_source_and_peer_budget() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let detached_parent = output_transaction(533);
    let parent = RawTxHash(detached_parent.hash());
    let child_tx = child_transaction(534, &detached_parent);
    let peer = ckb_network::PeerIndex::from(534);
    let child_admission =
        ValidatedAdmission::remote(child_tx, peer).expect("remote dependent fixture is valid");
    let child = child_admission.identity.raw.clone();
    let charged = child_admission.charge_for_foundation();
    apply_without_work(
        authority
            .plan_admission(child_admission)
            .expect("remote dependent enters preacceptance"),
    );
    assert_eq!(authority.resources().peer(peer), charged);

    let facts = ChainTransitionFacts::for_foundation(
        next_view(63),
        block_changes(Vec::new(), vec![detached_parent]),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("detached parent facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("detached origin finds the preaccepted dependent")
        .validate_for_foundation(Vec::new())
        .expect("dependency recovery needs no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("trusted parent and peer-owned dependent recover atomically"),
    );

    assert!(matches!(
        authority.entry(&parent),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
    ));
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                entry.source,
                PreAcceptedSource::Remote(remote)
                    if remote.residency.peer == peer
                        && remote.payload == RemotePayloadOrigin::IngressPeer
            )
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_eq!(authority.resources().peer(peer), charged);
    assert_resource_reference(&authority);
}

#[test]
fn uak_chain_recovery_keeps_affected_compute_settleable_across_the_cut() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let detached_parent = output_transaction(1_325);
    let parent = RawTxHash(detached_parent.hash());
    let parent_output = OutPoint::new(detached_parent.hash(), 0);
    let child_tx = child_transaction(1_326, &detached_parent);
    let child_admission = ValidatedAdmission::remote(child_tx, ckb_network::PeerIndex::from(1_326))
        .expect("active recovery child fixture is valid");
    let child = child_admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(child_admission)
            .expect("active recovery child enters preacceptance"),
    );
    let (_, work) = super::foundation::take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                &child,
                owner_version(&authority, &child),
                WorkPermit::ResolveOnly,
            )
            .expect("active recovery child checks out")
            .apply(),
    );
    let facts = ChainTransitionFacts::for_foundation(
        next_view(81),
        block_changes(Vec::new(), vec![detached_parent]),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("detached parent facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("dependency invalidation does not revoke active compute")
        .validate_for_foundation(Vec::new())
        .expect("recovery requires no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("parent recovery and chain cut preserve the child lease"),
    );

    assert!(matches!(
        authority.entry(&parent),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    apply_without_work(
        authority
            .apply_settlement(
                work.missing(vec![DependencyKey::Cell(parent_output)])
                    .expect("the old-view missing result is bounded"),
            )
            .expect("matching old-view completion remains settleable"),
    );
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
    assert_resource_reference(&authority);
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
    apply_without_work(
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
    apply_without_work(
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
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("detached facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("chain work distinguishes a direct detach from dependent requeue")
        .validate_for_foundation(Vec::new())
        .expect("recovery requires no proposal facts");
    apply_without_work(
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
    apply_without_work(
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
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("attached conflict facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("accepted conflict closure is bounded")
        .validate_for_foundation(Vec::new())
        .expect("conflict transition requires no proposal facts");
    apply_without_work(
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

#[test]
fn uak_chain_reconcile_demotes_gap_outside_the_new_window() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = output_transaction(541);
    let hash = accept_remote_transaction(
        &mut authority,
        transaction.clone(),
        541,
        AcceptedStatus::Gap,
        Vec::new(),
    );
    let proposal = ProposalId(transaction.proposal_short_id());
    let promoted_tx = output_transaction(544);
    let promoted = accept_remote_transaction(
        &mut authority,
        promoted_tx.clone(),
        544,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let promoted_proposal = ProposalId(promoted_tx.proposal_short_id());
    let facts = ChainTransitionFacts::for_foundation(
        next_view(55),
        block_changes(Vec::new(), Vec::new()),
        vec![proposal.clone(), promoted_proposal.clone()],
        Vec::new(),
        ChainPackagingMode::Package,
    )
    .expect("proposal change is canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("Gap owner is found through the proposal index")
        .validate_for_foundation(vec![
            (proposal, ProposalWindowPosition::Outside),
            (promoted_proposal, ProposalWindowPosition::Proposed),
        ])
        .expect("the new window position is exhaustive");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("Gap demotion and chain view install are atomic"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::Accepted(entry)) if entry.status() == AcceptedStatus::Pending
    ));
    assert!(matches!(
        authority.entry(&promoted),
        Some(OwnedTx::Accepted(entry)) if entry.status() == AcceptedStatus::Proposed
    ));
    assert_membership_reference(&authority);
}

#[test]
fn uak_chain_proposal_outside_demotes_remote_base_and_reactivates_its_deadline() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(1_725);
    let hash = admit_remote_until(&mut authority, 1_725, 725, 10);
    apply_without_work(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction.clone(), ProposalContextId(25))
                    .expect("proposal fixture is valid"),
            )
            .expect("remote owner promotes without losing its base"),
    );
    let version = owner_version(&authority, &hash);
    let proposal = ProposalId(transaction.proposal_short_id());
    let facts = ChainTransitionFacts::for_foundation(
        next_view(83),
        block_changes(Vec::new(), Vec::new()),
        vec![proposal.clone()],
        Vec::new(),
        ChainPackagingMode::Package,
    )
    .expect("proposal transition facts are canonical");
    let work = authority
        .chain_validation_work(facts)
        .expect("the proposal owner is selected through the proposal index");
    assert_eq!(
        work.required_proposals()
            .expect("the bounded proposal query fits"),
        vec![proposal.clone()]
    );
    let receipt = work
        .validate_for_foundation(vec![(proposal, ProposalWindowPosition::Outside)])
        .expect("the proposal position is exhaustive");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("source demotion and deadline publication are one Apply"),
    );

    assert_eq!(owner_version(&authority, &hash), version);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                entry.source,
                PreAcceptedSource::Remote(remote)
                    if remote.residency.expires_at == RemoteDeadline(10)
            )
    ));
    let expired = authority
        .plan_remote_expiry_for_foundation(
            RemoteDeadline(10),
            NonZeroUsize::new(1).expect("fixture slice is non-zero"),
        )
        .expect("reactivated deadline plans")
        .expect("demoted owner is immediately due")
        .apply();
    assert_eq!(expired.retired_len(), 1);
    assert!(authority.entry(&hash).is_none());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_chain_proposal_demotion_preserves_active_remote_compute_capability() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(1_726);
    let hash = admit_remote_until(&mut authority, 1_726, 726, 20);
    apply_without_work(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction.clone(), ProposalContextId(26))
                    .expect("proposal fixture is valid"),
            )
            .expect("remote owner promotes"),
    );
    let (_, work) = super::foundation::take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                &hash,
                owner_version(&authority, &hash),
                WorkPermit::ResolveOnly,
            )
            .expect("promoted owner checks out trusted work")
            .apply(),
    );
    let active_version = owner_version(&authority, &hash);
    let proposal = ProposalId(transaction.proposal_short_id());
    let facts = ChainTransitionFacts::for_foundation(
        next_view(84),
        block_changes(Vec::new(), Vec::new()),
        vec![proposal.clone()],
        Vec::new(),
        ChainPackagingMode::Package,
    )
    .expect("proposal transition facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("source-only demotion does not require compute drain")
        .validate_for_foundation(vec![(proposal, ProposalWindowPosition::Outside)])
        .expect("the final position is exhaustive");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("demotion preserves the unique active capability"),
    );

    assert_eq!(owner_version(&authority, &hash), active_version);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Remote(_))
                && matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    apply_without_work(
        authority
            .apply_settlement(work.rejected(super::super::state::RejectionKind::Policy))
            .expect("the pre-demotion work capability remains uniquely settleable"),
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_chain_trusted_proposal_expiry_publishes_definitive_parent_loss() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(1_727);
    let parent_admission = ValidatedAdmission::proposal(parent_tx.clone(), ProposalContextId(27))
        .expect("trusted proposal fixture is valid");
    let parent = parent_admission.identity.raw.clone();
    apply_without_work(
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
    apply_without_work(
        authority
            .plan_admission(child_admission)
            .expect("dependent child enters preacceptance"),
    );
    let (_, child_work) = super::foundation::take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                &child,
                owner_version(&authority, &child),
                WorkPermit::ResolveOnly,
            )
            .expect("dependent child checks out")
            .apply(),
    );
    let proposal = ProposalId(parent_tx.proposal_short_id());
    let facts = ChainTransitionFacts::for_foundation(
        ChainViewId::new(ChainRevision(1), Byte32::zero()),
        block_changes(Vec::new(), Vec::new()),
        vec![proposal.clone()],
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("same-tip proposal transition facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("trusted proposal expiry is bounded")
        .validate_for_foundation(vec![(proposal, ProposalWindowPosition::Outside)])
        .expect("the final proposal position is exhaustive");
    apply_without_work(
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
    apply_without_work(
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
fn uak_chain_trusted_proposal_expiry_requires_only_its_active_owner_to_drain() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(1_729);
    let admission = ValidatedAdmission::proposal(transaction.clone(), ProposalContextId(29))
        .expect("trusted proposal fixture is valid");
    let hash = admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(admission)
            .expect("trusted proposal enters preacceptance"),
    );
    let _work = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::ResolveOnly,
        )
        .expect("trusted proposal checks out")
        .apply();
    let proposal = ProposalId(transaction.proposal_short_id());
    let facts = ChainTransitionFacts::for_foundation(
        next_view(85),
        block_changes(Vec::new(), Vec::new()),
        vec![proposal.clone()],
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("proposal transition facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("read-only validation does not cancel active work")
        .validate_for_foundation(vec![(proposal, ProposalWindowPosition::Outside)])
        .expect("the final proposal position is exhaustive");
    let before = authority.normalized_snapshot();
    assert_eq!(
        authority.plan_chain_transition(receipt).err(),
        Some(PlanError::Backpressure(Backpressure::ActiveWorkDrain))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_chain_receipt_detects_same_version_proposal_source_refresh() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(1_730);
    let hash = admit_remote_until(&mut authority, 1_730, 730, 20);
    apply_without_work(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction.clone(), ProposalContextId(30))
                    .expect("initial proposal fixture is valid"),
            )
            .expect("remote owner promotes"),
    );
    let version = owner_version(&authority, &hash);
    let proposal = ProposalId(transaction.proposal_short_id());
    let facts = ChainTransitionFacts::for_foundation(
        next_view(86),
        block_changes(Vec::new(), Vec::new()),
        vec![proposal.clone()],
        Vec::new(),
        ChainPackagingMode::Package,
    )
    .expect("proposal transition facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("proposal source is captured in the exact expectation")
        .validate_for_foundation(vec![(proposal, ProposalWindowPosition::Outside)])
        .expect("the final proposal position is exhaustive");

    apply_without_work(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction, ProposalContextId(31))
                    .expect("refreshed proposal fixture is valid"),
            )
            .expect("same-witness lease refresh preserves EntryVersion"),
    );
    assert_eq!(owner_version(&authority, &hash), version);
    let before = authority.normalized_snapshot();
    assert_eq!(
        authority.plan_chain_transition(receipt).err(),
        Some(PlanError::Stale(StalePlan::Version))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                entry.source,
                PreAcceptedSource::Proposal {
                    lease,
                    base: ProposalBase::Remote(_),
                } if lease.context == ProposalContextId(31)
            )
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_detached_proposal_does_not_cancel_preaccepted_compute() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(1_323);
    let parent = accept_remote_transaction(
        &mut authority,
        parent_tx.clone(),
        1_323,
        AcceptedStatus::Proposed,
        Vec::new(),
    );
    let parent_proposal = ProposalId(parent_tx.proposal_short_id());
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let child_tx = child_transaction(1_324, &parent_tx);
    let child_admission = ValidatedAdmission::remote(child_tx, ckb_network::PeerIndex::from(1_324))
        .expect("preaccepted child fixture is valid");
    let child = child_admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(child_admission)
            .expect("preaccepted child enters resolve queue"),
    );
    let (_, work) = super::foundation::take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                &child,
                owner_version(&authority, &child),
                WorkPermit::ResolveOnly,
            )
            .expect("preaccepted child checks out")
            .apply(),
    );

    let facts = ChainTransitionFacts::for_foundation(
        next_view(80),
        block_changes(Vec::new(), Vec::new()),
        Vec::new(),
        vec![parent_proposal.clone()],
        ChainPackagingMode::ObserveOnly,
    )
    .expect("detached proposal facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("proposal demotion does not require a compute drain")
        .validate_for_foundation(vec![(parent_proposal, ProposalWindowPosition::Outside)])
        .expect("the parent proposal is outside the new window");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("status demotion preserves active preaccepted compute"),
    );

    assert!(matches!(
        authority.entry(&parent),
        Some(OwnedTx::Accepted(entry)) if entry.status() == AcceptedStatus::Pending
    ));
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    apply_without_work(
        authority
            .apply_settlement(
                work.missing(vec![DependencyKey::Cell(parent_output)])
                    .expect("the old-view missing result is bounded"),
            )
            .expect("matching completion releases its lease after the chain cut"),
    );
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
    assert_resource_reference(&authority);
}

#[test]
fn uak_chain_receipt_stales_on_accepted_change_but_not_unrelated_preacceptance() {
    let mut stale_authority = TxPoolAuthority::for_foundation(limits());
    let facts = ChainTransitionFacts::for_foundation(
        next_view(56),
        block_changes(Vec::new(), Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("empty facts are canonical");
    let stale_receipt = stale_authority
        .chain_validation_work(facts)
        .expect("empty work captures source versions")
        .validate_for_foundation(Vec::new())
        .expect("empty work validates");
    accept_remote_transaction(
        &mut stale_authority,
        output_transaction(542),
        542,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let before = stale_authority.normalized_snapshot();
    assert_eq!(
        stale_authority.plan_chain_transition(stale_receipt).err(),
        Some(PlanError::Stale(StalePlan::SourceVersion))
    );
    assert_eq!(stale_authority.normalized_snapshot(), before);

    let mut compatible_authority = TxPoolAuthority::for_foundation(limits());
    let facts = ChainTransitionFacts::for_foundation(
        next_view(57),
        block_changes(Vec::new(), Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("empty facts are canonical");
    let receipt = compatible_authority
        .chain_validation_work(facts)
        .expect("empty work captures accepted sources only")
        .validate_for_foundation(Vec::new())
        .expect("empty work validates");
    let unrelated = admit_remote(&mut compatible_authority, 543, 543);
    apply_without_work(
        compatible_authority
            .plan_chain_transition(receipt)
            .expect("unrelated preacceptance does not stale a ChainPlan"),
    );
    assert!(compatible_authority.entry(&unrelated).is_some());
    assert_eq!(compatible_authority.chain_revision(), ChainRevision(1));
}

#[test]
fn uak_chain_recovery_receipt_proves_targeted_vacancy() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = output_transaction(545);
    let facts = ChainTransitionFacts::for_foundation(
        next_view(74),
        block_changes(Vec::new(), vec![transaction.clone()]),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
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
    apply_without_work(
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
fn uak_chain_work_requires_targeted_active_owner_drain_and_never_applies_a_prefix() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 551, 551);
    let version = owner_version(&authority, &hash);
    let _active = authority
        .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
        .expect("affected owner checks out")
        .apply();
    let transaction = authority
        .entry(&hash)
        .expect("owner remains")
        .record()
        .tx
        .as_ref()
        .clone();
    let facts = ChainTransitionFacts::for_foundation(
        next_view(58),
        block_changes(vec![transaction], Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("attached active owner is canonical");
    let before = authority.normalized_snapshot();
    let receipt = authority
        .chain_validation_work(facts)
        .expect("read-only validation may capture an active owner")
        .validate_for_foundation(Vec::new())
        .expect("the committed hash needs no proposal lookup");
    assert_eq!(
        authority.plan_chain_transition(receipt).err(),
        Some(PlanError::Backpressure(Backpressure::ActiveWorkDrain))
    );
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_large_independent_chain_facts_do_not_consume_the_causal_bound() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let attached = (0..=crate::constants::MAX_POOL_MUTATION_CANDIDATES)
        .map(|offset| output_transaction(600 + offset as u32))
        .collect();
    let facts = ChainTransitionFacts::for_foundation(
        next_view(59),
        block_changes(attached, Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("the block facts themselves remain canonical");
    let work = authority
        .chain_validation_work(facts)
        .expect("block-bounded independent facts do not consume the causal K bound");
    assert!(
        work.required_proposals()
            .expect("proposal list fits")
            .is_empty()
    );
    let receipt = work
        .validate_for_foundation(Vec::new())
        .expect("independent facts require no proposal lookups");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("the empty affected owner slice installs atomically"),
    );
    assert_eq!(authority.chain_revision(), ChainRevision(1));
    assert_eq!(authority.owner_count(), 0);
}

#[test]
fn uak_direct_detached_owners_do_not_consume_the_causal_bound() {
    let mut authority = TxPoolAuthority::for_foundation(large_chain_limits());
    let mut detached = Vec::new();
    detached
        .try_reserve(crate::constants::MAX_POOL_MUTATION_CANDIDATES + 1)
        .expect("fixture vector is small");
    for offset in 0..=crate::constants::MAX_POOL_MUTATION_CANDIDATES {
        let transaction = output_transaction(1_000 + offset as u32);
        accept_remote_transaction(
            &mut authority,
            transaction.clone(),
            1_000 + offset,
            AcceptedStatus::Pending,
            Vec::new(),
        );
        detached.push(transaction);
    }
    let facts = ChainTransitionFacts::for_foundation(
        next_view(67),
        block_changes(Vec::new(), detached),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("detached block facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("direct block owners do not consume the independent causal bound")
        .validate_for_foundation(Vec::new())
        .expect("direct recoveries require no proposal facts");
    apply_without_work(
        authority
            .plan_chain_transition(receipt)
            .expect("the block-bounded recovery set installs atomically"),
    );
    assert_eq!(authority.membership_counts().pending, 0);
    assert_eq!(
        authority.owner_count(),
        crate::constants::MAX_POOL_MUTATION_CANDIDATES + 1
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_unrepresentable_recovery_set_requires_generation_replacement() {
    let constrained = ResourceLimits::new(
        ResourceVector::new(1, 64 * 1024, 64, 1),
        ResourceVector::new(1, 64 * 1024, 64, 1),
        ResourceVector::new(1, 64 * 1024, 64, 1),
        AcceptedResources::new(4, 256 * 1024, 256 * 1024, 1_000),
        ComputeLimits::new(64 * 1024, 64 * 1024, 64),
    )
    .expect("one-at-a-time ingress and larger accepted membership are valid");
    let mut authority = TxPoolAuthority::for_foundation(constrained);
    let left = output_transaction(1_301);
    let right = output_transaction(1_302);
    for (peer, transaction) in [(1_301, left.clone()), (1_302, right.clone())] {
        accept_remote_transaction(
            &mut authority,
            transaction,
            peer,
            AcceptedStatus::Pending,
            Vec::new(),
        );
    }
    let facts = ChainTransitionFacts::for_foundation(
        next_view(69),
        block_changes(Vec::new(), vec![left, right]),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("detached recovery facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("block facts fit the causal bound")
        .validate_for_foundation(Vec::new())
        .expect("recoveries require no proposal facts");
    let before = authority.normalized_snapshot();
    assert_eq!(
        authority.plan_chain_transition(receipt).err(),
        Some(PlanError::Backpressure(Backpressure::GenerationReplacement))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert_resource_reference(&authority);
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
    let facts = ChainTransitionFacts::for_foundation(
        next_view(61),
        block_changes(vec![attached.build()], Vec::new()),
        Vec::new(),
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("large hostile block facts remain canonical");
    let before = authority.normalized_snapshot();
    assert_eq!(
        authority.chain_validation_work(facts).err(),
        Some(PlanError::Backpressure(Backpressure::GenerationReplacement))
    );
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_empty_chain_transition_updates_only_the_chain_cut() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let before_occ = authority.source_versions_for_reference();
    let before = authority.template_source_versions_for_reference();
    let committed = empty_transition(&mut authority, 60);
    let after_occ = authority.source_versions_for_reference();
    let after = authority.template_source_versions_for_reference();
    assert!(committed.handoff_is_none());
    assert_eq!(authority.chain_revision(), ChainRevision(1));
    assert!(after.chain > before.chain);
    assert_eq!(after_occ, before_occ);
    assert_eq!(after.proposals, before.proposals);
    assert_eq!(after.transactions, before.transactions);
    assert!(authority.primary_projection_consistent());
}
