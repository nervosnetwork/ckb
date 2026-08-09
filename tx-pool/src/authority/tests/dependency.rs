use super::super::{
    plan::{DirectAdmissionDisposition, PlanError, SettlementBatch, StalePlan, TxPoolAuthority},
    resources::{
        AcceptedResources, ComputeLimits, ResidencyPolicy, ResourceLimits, ResourceVector,
    },
    runtime::{AuthorityMaintenanceOutcome, AuthorityRuntime},
    state::{
        AcceptedStatus, DependencyKey, EntryVersion, OwnedTx, PoolGeneration, PreAcceptedPhase,
        QueuedWork, RawTxHash, ResolvedPayload, TxIdentity, ValidatedAdmission, VerifyCapability,
        WorkPermit,
    },
    work::{CheckedOutWork, ComputeSettlement, ContinuousResolution, ContinuousResolveWork},
};
use super::foundation::{FixtureCommit, direct_verified_facts, genesis_snapshot, runtime_config};
use ckb_network::PeerIndex;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, FeeRate, TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
    sync::Arc,
};

fn limits() -> ResourceLimits {
    ResourceLimits::new(
        ResourceVector::new(12, 96 * 1024, 96, 8),
        ResourceVector::new(8, 64 * 1024, 64, 6),
        ResourceVector::new(4, 32 * 1024, 32, 4),
        AcceptedResources::new(12, 96 * 1024, 96 * 1024, 96),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .expect("dependency fixture limits admit one indivisible grant")
}

fn output_transaction(version: u32) -> TransactionView {
    TransactionBuilder::default()
        .version(version)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build()
}

fn input_transaction(version: u32, input: OutPoint) -> TransactionView {
    TransactionBuilder::default()
        .version(version)
        .input(CellInput::new(input, 0))
        .build()
}

fn apply_plan(commit: impl FixtureCommit) {
    drop(commit.into_committed());
}

fn owner_version(authority: &TxPoolAuthority, hash: &RawTxHash) -> EntryVersion {
    authority
        .entry(hash)
        .expect("dependency fixture owner exists")
        .record()
        .version
}

fn admit(authority: &mut TxPoolAuthority, admission: ValidatedAdmission) -> RawTxHash {
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("dependency fixture admission plans"),
    );
    hash
}

fn checkout_resolve(
    authority: &mut TxPoolAuthority,
    hash: &RawTxHash,
) -> super::super::work::ResolveWork {
    let committed = authority
        .plan_checkout_for_foundation(
            hash,
            owner_version(authority, hash),
            WorkPermit::ResolveOnly,
        )
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(work) = committed.into_work() else {
        panic!("resolve-only capability returned another work type");
    };
    work
}

fn checkout_continuous(authority: &mut TxPoolAuthority, hash: &RawTxHash) -> ContinuousResolveWork {
    let committed = authority
        .plan_checkout_for_foundation(
            hash,
            owner_version(authority, hash),
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("continuous checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(work) = committed.into_work() else {
        panic!("continuous capability returned another work type");
    };
    work
}

fn verified_settlement(
    resolve: ContinuousResolveWork,
    expanded_dependencies: Vec<OutPoint>,
    chain_inputs: Vec<OutPoint>,
) -> ComputeSettlement {
    verified_settlement_with_fee(
        resolve,
        expanded_dependencies,
        chain_inputs,
        Capacity::shannons(1),
    )
}

fn verified_settlement_with_fee(
    resolve: ContinuousResolveWork,
    expanded_dependencies: Vec<OutPoint>,
    chain_inputs: Vec<OutPoint>,
    fee: Capacity,
) -> ComputeSettlement {
    let transaction = resolve.transaction().clone();
    let resident_bytes = transaction.data().total_size();
    // Every dependency fixture in this module is pool-backed. Expanded and
    // declared dependency roles are content facts; they must not be stamped
    // as chain-location evidence merely because resolution succeeded.
    let chain_dependencies = Vec::new();
    let payload = ResolvedPayload::for_foundation(
        &transaction,
        expanded_dependencies,
        resolve.resolution_grant().max_edges(),
        fee,
        resident_bytes,
        chain_inputs,
        chain_dependencies,
    )
    .expect("dependency fixture resolution evidence is valid");
    let ContinuousResolution::Verify(verify) = resolve
        .into_verify(payload)
        .expect("dependency fixture payload matches its checked-out transaction")
    else {
        panic!("dependency fixture fits the continuous compute grant");
    };
    verify.verified(0)
}

fn accept_remote(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    peer: usize,
    chain_inputs: Vec<OutPoint>,
    fee: Capacity,
) -> RawTxHash {
    let hash = admit(
        authority,
        ValidatedAdmission::remote(transaction, PeerIndex::from(peer))
            .expect("accepted dependency fixture admission is valid"),
    );
    let settlement = verified_settlement_with_fee(
        checkout_continuous(authority, &hash),
        Vec::new(),
        chain_inputs,
        fee,
    );
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("accepted dependency fixture verifies"),
    );
    let version = owner_version(authority, &hash);
    apply_plan(
        authority
            .plan_accept_for_foundation(&hash, version, AcceptedStatus::Pending)
            .expect("accepted dependency fixture enters membership"),
    );
    hash
}

fn drain_dependency_maintenance(authority: &mut TxPoolAuthority) -> usize {
    authority
        .drain_dependency_maintenance_for_foundation()
        .expect("the stable dependency rank strictly decreases to zero")
        .iter()
        .filter(|step| step.owner_requeued())
        .count()
}

#[test]
fn uak_runtime_dependency_maintenance_is_one_level_triggered_step() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let hash = seed_runtime_dependency_maintenance(&runtime);

    assert_eq!(
        runtime
            .maintain_dependency()
            .expect("the first bounded step requeues one owner"),
        AuthorityMaintenanceOutcome::Applied
    );
    assert_eq!(
        runtime
            .maintain_dependency()
            .expect("the second bounded step completes the dirty key"),
        AuthorityMaintenanceOutcome::Applied
    );
    assert_eq!(
        runtime
            .maintain_dependency()
            .expect("the level-triggered frontier becomes idle"),
        AuthorityMaintenanceOutcome::Idle
    );
    runtime.with_authority_for_foundation(|authority| {
        assert!(matches!(
            authority.entry(&hash),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
        ));
        assert!(
            authority
                .dependency_maintenance_observation_for_foundation()
                .expect("the dependency projection remains readable")
                .is_none()
        );
        assert!(authority.primary_projection_consistent());
    });
}

pub(super) fn seed_runtime_dependency_maintenance(runtime: &AuthorityRuntime) -> RawTxHash {
    runtime.with_authority_for_foundation(|authority| {
        let dependency = OutPoint::new(Byte32::new([0x9a; 32]), 0);
        let key = DependencyKey::Cell(dependency.clone());
        let hash = admit(
            authority,
            ValidatedAdmission::remote(input_transaction(699, dependency), PeerIndex::from(89))
                .expect("runtime dependency admission is valid"),
        );
        let work = checkout_resolve(authority, &hash);
        apply_plan(
            authority
                .apply_settlement(
                    work.missing(vec![key.clone()])
                        .expect("the missing edge fits the compute grant"),
                )
                .expect("the missing owner enters dependency wait"),
        );
        apply_plan(
            authority
                .plan_dependency_availability_for_foundation(vec![key.clone()])
                .expect("availability event planning is valid")
                .expect("the live waiter creates one dirty level"),
        );
        hash
    })
}

#[test]
fn uak_dependency_level_requeues_or_terminalizes_once() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let dependency = OutPoint::new(Byte32::new([0xa1; 32]), 0);
    let key = DependencyKey::Cell(dependency.clone());
    let transaction = input_transaction(701, dependency);
    let hash = admit(
        &mut authority,
        ValidatedAdmission::remote(transaction, PeerIndex::from(91))
            .expect("remote dependency fixture is valid"),
    );

    let first_work = checkout_resolve(&mut authority, &hash);
    let before_dropped_event = authority.normalized_snapshot();
    drop(
        authority
            .plan_dependency_availability_for_foundation(vec![key.clone()])
            .expect("availability event plans")
            .expect("a live consumer records the event"),
    );
    assert_eq!(authority.normalized_snapshot(), before_dropped_event);

    apply_plan(
        authority
            .plan_dependency_availability_for_foundation(vec![key.clone()])
            .expect("availability event plans")
            .expect("a live consumer records the event"),
    );
    apply_plan(
        authority
            .apply_settlement(
                first_work
                    .missing(vec![key.clone()])
                    .expect("missing receipt is bounded"),
            )
            .expect("a raced missing receipt settles by requeueing"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));

    let second_work = checkout_resolve(&mut authority, &hash);
    apply_plan(
        authority
            .apply_settlement(
                second_work
                    .missing(vec![key.clone()])
                    .expect("missing receipt is bounded"),
            )
            .expect("post-event missing receipt registers a wait"),
    );
    let waiting_version = owner_version(&authority, &hash);
    for _ in 0..2 {
        apply_plan(
            authority
                .plan_dependency_availability_for_foundation(vec![key.clone()])
                .expect("repeated availability plans")
                .expect("the waiter keeps the level live"),
        );
    }

    let maintenance = authority
        .drain_dependency_maintenance_for_foundation()
        .expect("coalesced levels strictly decrease the stable dependency rank");
    assert_eq!(
        maintenance
            .iter()
            .filter(|step| step.owner_requeued())
            .count(),
        1
    );
    assert_eq!(maintenance[0].before_rank().value(), 4);
    assert_eq!(maintenance[0].after_rank().value(), 2);
    assert_eq!(
        owner_version(&authority, &hash),
        EntryVersion(waiting_version.0 + 1),
        "coalesced levels perform one physical primary requeue"
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_parent_acceptance_publishes_output_availability_atomically() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(702);
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let key = DependencyKey::Cell(parent_output.clone());
    let parent = admit(
        &mut authority,
        ValidatedAdmission::proposal(parent_tx).expect("parent proposal is valid"),
    );
    let child = admit(
        &mut authority,
        ValidatedAdmission::remote(input_transaction(703, parent_output), PeerIndex::from(109))
            .expect("waiting child is valid"),
    );
    let missing = checkout_resolve(&mut authority, &child)
        .missing(vec![key])
        .expect("missing output receipt is bounded");
    apply_plan(
        authority
            .apply_settlement(missing)
            .expect("child registers its exact wait"),
    );
    let waiting_version = owner_version(&authority, &child);

    let verified = verified_settlement(
        checkout_continuous(&mut authority, &parent),
        Vec::new(),
        Vec::new(),
    );
    apply_plan(
        authority
            .apply_settlement(verified)
            .expect("parent verification settles"),
    );
    let parent_version = owner_version(&authority, &parent);
    apply_plan(
        authority
            .plan_accept_for_foundation(&parent, parent_version, AcceptedStatus::Pending)
            .expect("parent membership and availability share one Apply"),
    );

    assert_eq!(drain_dependency_maintenance(&mut authority), 1);
    assert_ne!(owner_version(&authority, &child), waiting_version);
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_direct_parent_acceptance_publishes_output_availability_atomically() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(704);
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let key = DependencyKey::Cell(parent_output.clone());
    let child = admit(
        &mut authority,
        ValidatedAdmission::remote(input_transaction(705, parent_output), PeerIndex::from(110))
            .expect("waiting child is valid"),
    );
    let missing = checkout_resolve(&mut authority, &child)
        .missing(vec![key])
        .expect("missing output receipt is bounded");
    apply_plan(
        authority
            .apply_settlement(missing)
            .expect("child registers its exact wait"),
    );
    let waiting_version = owner_version(&authority, &child);

    let verified = direct_verified_facts(
        &parent_tx,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1_000),
    );
    let disposition = authority
        .plan_direct_admission_for_foundation(
            Arc::new(parent_tx),
            verified,
            AcceptedStatus::Pending,
        )
        .expect("direct parent membership and availability share one Plan");
    let DirectAdmissionDisposition::Accepted(plan) = disposition else {
        panic!("vacant direct parent must become Accepted");
    };
    apply_plan(plan);

    assert_eq!(drain_dependency_maintenance(&mut authority), 1);
    assert_ne!(owner_version(&authority, &child), waiting_version);
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_coalesced_loss_then_availability_wakes_a_post_loss_waiter() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(709);
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let key = DependencyKey::Cell(parent_output.clone());
    let parent = admit(
        &mut authority,
        ValidatedAdmission::proposal(parent_tx).expect("coalesced-event parent proposal is valid"),
    );
    let early_child = admit(
        &mut authority,
        ValidatedAdmission::remote(
            input_transaction(708, parent_output.clone()),
            PeerIndex::from(107),
        )
        .expect("coalesced-event remote child is valid"),
    );
    let early_missing = checkout_resolve(&mut authority, &early_child)
        .missing(vec![key.clone()])
        .expect("early missing receipt is bounded");
    apply_plan(
        authority
            .apply_settlement(early_missing)
            .expect("the early remote child registers its wait"),
    );
    apply_plan(
        authority
            .plan_dependency_availability_for_foundation(vec![key.clone()])
            .expect("the first availability change plans")
            .expect("the early waiter creates an active dirty traversal"),
    );

    apply_plan(
        authority
            .plan_terminalize_for_foundation(&parent, owner_version(&authority, &parent))
            .expect("parent loss coalesces behind the active traversal"),
    );
    let late_child = admit(
        &mut authority,
        ValidatedAdmission::remote(input_transaction(707, parent_output), PeerIndex::from(108))
            .expect("post-loss remote child is valid"),
    );
    let post_loss_missing = checkout_resolve(&mut authority, &late_child)
        .missing(vec![key.clone()])
        .expect("post-loss missing receipt is bounded");
    apply_plan(
        authority
            .apply_settlement(post_loss_missing)
            .expect("remote child may wait for refetch"),
    );
    let early_waiting_version = owner_version(&authority, &early_child);
    let late_waiting_version = owner_version(&authority, &late_child);

    apply_plan(
        authority
            .plan_dependency_availability_for_foundation(vec![key])
            .expect("new availability coalesces into the pending loss")
            .expect("the waiter keeps the exact level indexed"),
    );
    assert_eq!(drain_dependency_maintenance(&mut authority), 2);
    assert_ne!(
        owner_version(&authority, &early_child),
        early_waiting_version
    );
    assert_ne!(owner_version(&authority, &late_child), late_waiting_version);
    for child in [&early_child, &late_child] {
        assert!(matches!(
            authority.entry(child),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
        ));
    }
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_parent_terminalization_cannot_strand_trusted_child() {
    for recovery in [false, true] {
        let mut authority = TxPoolAuthority::for_foundation(limits());
        let parent_tx = output_transaction(if recovery { 711 } else { 710 });
        let child_tx = input_transaction(
            if recovery { 713 } else { 712 },
            OutPoint::new(parent_tx.hash(), 0),
        );
        let parent_admission = if recovery {
            ValidatedAdmission::recovery(parent_tx.clone(), PoolGeneration(0))
        } else {
            ValidatedAdmission::proposal(parent_tx.clone())
        }
        .expect("trusted parent admission is valid");
        let child_admission = if recovery {
            ValidatedAdmission::recovery(child_tx.clone(), PoolGeneration(0))
        } else {
            ValidatedAdmission::proposal(child_tx.clone())
        }
        .expect("trusted child admission is valid");
        let parent = admit(&mut authority, parent_admission);
        let child = admit(&mut authority, child_admission);
        let settlement =
            verified_settlement(checkout_continuous(&mut authority, &child), vec![], vec![]);

        if recovery {
            let parent_version = owner_version(&authority, &parent);
            apply_plan(
                authority
                    .plan_terminalize_for_foundation(&parent, parent_version)
                    .expect("definitive parent terminalization plans"),
            );
            apply_plan(
                authority
                    .apply_settlement(settlement)
                    .expect("stale worker evidence atomically requeues"),
            );
            assert_eq!(drain_dependency_maintenance(&mut authority), 0);
        } else {
            apply_plan(
                authority
                    .apply_settlement(settlement)
                    .expect("verified child settlement plans"),
            );
            let parent_version = owner_version(&authority, &parent);
            apply_plan(
                authority
                    .plan_terminalize_for_foundation(&parent, parent_version)
                    .expect("definitive parent terminalization plans"),
            );
            let version = owner_version(&authority, &child);
            let before_stale_accept = authority.normalized_snapshot();
            assert_eq!(
                authority
                    .plan_accept_for_foundation(&child, version, AcceptedStatus::Pending)
                    .err(),
                Some(PlanError::Stale(StalePlan::Dependency))
            );
            assert_eq!(authority.normalized_snapshot(), before_stale_accept);
            assert_eq!(drain_dependency_maintenance(&mut authority), 1);
        }

        assert!(matches!(
            authority.entry(&child),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
        ));

        let missing_parent = DependencyKey::Cell(OutPoint::new(parent_tx.hash(), 0));
        let second_resolution = checkout_resolve(&mut authority, &child)
            .missing(vec![missing_parent])
            .expect("the definitive missing dependency fits the grant");
        apply_plan(
            authority
                .apply_settlement(second_resolution)
                .expect("trusted definitive loss reaches a terminal outcome"),
        );
        assert!(authority.entry(&child).is_none());
        assert!(authority.primary_projection_consistent());
    }
}

#[test]
fn uak_dependency_maintenance_never_revokes_active_compute_capability() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(717);
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let dependency = DependencyKey::Cell(parent_output.clone());
    let parent = admit(
        &mut authority,
        ValidatedAdmission::proposal(parent_tx).expect("active-consumer parent admission is valid"),
    );
    let child = admit(
        &mut authority,
        ValidatedAdmission::remote(input_transaction(718, parent_output), PeerIndex::from(106))
            .expect("active dependency consumer is valid"),
    );
    let work = checkout_resolve(&mut authority, &child);

    apply_plan(
        authority
            .plan_terminalize_for_foundation(&parent, owner_version(&authority, &parent))
            .expect("parent loss publishes one definitive dependency cut"),
    );
    assert_eq!(
        drain_dependency_maintenance(&mut authority),
        0,
        "maintenance advances the dirty cursor without stealing checked-out work"
    );
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));

    apply_plan(
        authority
            .apply_settlement(
                work.missing(vec![dependency])
                    .expect("the matching old-cut result is bounded"),
            )
            .expect("the unique completion remains settleable"),
    );
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_batch_acceptance_cannot_bypass_dependency_cut() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(714);
    let child_tx = input_transaction(715, OutPoint::new(parent_tx.hash(), 0));
    let unrelated_input = OutPoint::new(Byte32::new([0xb1; 32]), 0);
    let unrelated_tx = input_transaction(716, unrelated_input.clone());
    let parent = admit(
        &mut authority,
        ValidatedAdmission::proposal(parent_tx).expect("parent proposal is valid"),
    );
    let child = admit(
        &mut authority,
        ValidatedAdmission::proposal(child_tx).expect("child proposal is valid"),
    );
    let unrelated = admit(
        &mut authority,
        ValidatedAdmission::remote(unrelated_tx, PeerIndex::from(105))
            .expect("unrelated admission is valid"),
    );
    let child_settlement = verified_settlement(
        checkout_continuous(&mut authority, &child),
        Vec::new(),
        Vec::new(),
    );
    apply_plan(
        authority
            .apply_settlement(child_settlement)
            .expect("child verification settles"),
    );
    let unrelated_settlement = verified_settlement(
        checkout_continuous(&mut authority, &unrelated),
        Vec::new(),
        vec![unrelated_input],
    );
    apply_plan(
        authority
            .apply_settlement(unrelated_settlement)
            .expect("unrelated verification settles"),
    );

    let parent_version = owner_version(&authority, &parent);
    apply_plan(
        authority
            .plan_terminalize_for_foundation(&parent, parent_version)
            .expect("parent terminalization publishes loss"),
    );
    let batch = SettlementBatch::new(vec![
        authority
            .independent_candidate_for_foundation(
                &child,
                owner_version(&authority, &child),
                AcceptedStatus::Pending,
            )
            .expect("child candidate has current final evidence"),
        authority
            .independent_candidate_for_foundation(
                &unrelated,
                owner_version(&authority, &unrelated),
                AcceptedStatus::Pending,
            )
            .expect("unrelated candidate has current final evidence"),
    ])
    .expect("two distinct candidates form a bounded batch");
    let before = authority.normalized_snapshot();
    assert_eq!(
        authority.plan_settlement(&batch).err(),
        Some(PlanError::Stale(StalePlan::Dependency))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_unindexed_expansion_loss_cannot_validate_old_resolution() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(720);
    let chain_input = OutPoint::new(Byte32::new([0xb2; 32]), 0);
    let child_tx = input_transaction(721, chain_input.clone());
    let parent = admit(
        &mut authority,
        ValidatedAdmission::proposal(parent_tx.clone()).expect("parent proposal is valid"),
    );
    let child = admit(
        &mut authority,
        ValidatedAdmission::remote(child_tx, PeerIndex::from(92))
            .expect("child admission is valid"),
    );
    let settlement = verified_settlement(
        checkout_continuous(&mut authority, &child),
        vec![OutPoint::new(parent_tx.hash(), 0)],
        vec![chain_input],
    );

    let parent_version = owner_version(&authority, &parent);
    apply_plan(
        authority
            .plan_terminalize_for_foundation(&parent, parent_version)
            .expect("parent terminalization publishes every output loss"),
    );
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("unindexed old resolution is consumed into requeue"),
    );
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_availability_does_not_invalidate_positive_dependency_evidence() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let chain_input = OutPoint::new(Byte32::new([0xc3; 32]), 0);
    let key = DependencyKey::Cell(chain_input.clone());
    let transaction = input_transaction(730, chain_input.clone());
    let hash = admit(
        &mut authority,
        ValidatedAdmission::remote(transaction, PeerIndex::from(93))
            .expect("positive-evidence admission is valid"),
    );
    let settlement = verified_settlement(
        checkout_continuous(&mut authority, &hash),
        Vec::new(),
        vec![chain_input],
    );
    apply_plan(
        authority
            .plan_dependency_availability_for_foundation(vec![key])
            .expect("availability event plans")
            .expect("the active resolver is an indexed consumer"),
    );
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("positive evidence survives availability"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Ready(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_dependency_loss_is_exact_key_scoped() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(740);
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let dependent_tx = input_transaction(741, parent_output);
    let unrelated_input = OutPoint::new(Byte32::new([0xd4; 32]), 0);
    let unrelated_tx = input_transaction(742, unrelated_input.clone());
    let parent = admit(
        &mut authority,
        ValidatedAdmission::proposal(parent_tx).expect("parent proposal is valid"),
    );
    let _dependent = admit(
        &mut authority,
        ValidatedAdmission::proposal(dependent_tx).expect("dependent proposal is valid"),
    );
    let unrelated = admit(
        &mut authority,
        ValidatedAdmission::remote(unrelated_tx, PeerIndex::from(94))
            .expect("unrelated admission is valid"),
    );
    let unrelated_settlement = verified_settlement(
        checkout_continuous(&mut authority, &unrelated),
        Vec::new(),
        vec![unrelated_input],
    );
    apply_plan(
        authority
            .apply_settlement(unrelated_settlement)
            .expect("unrelated verification settles"),
    );
    let parent_version = owner_version(&authority, &parent);
    apply_plan(
        authority
            .plan_terminalize_for_foundation(&parent, parent_version)
            .expect("parent terminalization plans"),
    );
    let version = owner_version(&authority, &unrelated);
    apply_plan(
        authority
            .plan_accept_for_foundation(&unrelated, version, AcceptedStatus::Pending)
            .expect("loss of key A does not invalidate key B"),
    );
    assert!(matches!(
        authority.entry(&unrelated),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_membership_removal_publishes_dependency_loss_atomically() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1));
    let chain_input = OutPoint::new(Byte32::new([0xe5; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(750u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let victim = accept_remote(
        &mut authority,
        victim_tx.clone(),
        95,
        vec![chain_input.clone()],
        Capacity::shannons(1),
    );
    let child_tx = input_transaction(751, OutPoint::new(victim_tx.hash(), 0));
    let child = admit(
        &mut authority,
        ValidatedAdmission::proposal(child_tx).expect("preaccepted child proposal is valid"),
    );
    let child_settlement = verified_settlement(
        checkout_continuous(&mut authority, &child),
        Vec::new(),
        Vec::new(),
    );
    apply_plan(
        authority
            .apply_settlement(child_settlement)
            .expect("preaccepted child verifies"),
    );

    let replacement_tx = TransactionBuilder::default()
        .version(752u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let replacement = admit(
        &mut authority,
        ValidatedAdmission::remote(replacement_tx, PeerIndex::from(96))
            .expect("replacement admission is valid"),
    );
    let replacement_settlement = verified_settlement_with_fee(
        checkout_continuous(&mut authority, &replacement),
        Vec::new(),
        vec![chain_input],
        Capacity::shannons(1_000_000),
    );
    apply_plan(
        authority
            .apply_settlement(replacement_settlement)
            .expect("replacement verifies"),
    );
    let replacement_version = owner_version(&authority, &replacement);
    let committed = authority
        .plan_accept_for_foundation(&replacement, replacement_version, AcceptedStatus::Pending)
        .expect("replacement membership plan is valid")
        .apply();
    assert!(
        committed
            .removals
            .iter()
            .any(|removal| removal.hash == victim)
    );
    assert!(authority.entry(&victim).is_none());

    let child_version = owner_version(&authority, &child);
    assert_eq!(
        authority
            .plan_accept_for_foundation(&child, child_version, AcceptedStatus::Pending)
            .err(),
        Some(PlanError::Stale(StalePlan::Dependency))
    );
    assert_eq!(drain_dependency_maintenance(&mut authority), 1);
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_membership_loss_removes_accepted_dependency_readers_before_publish() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1));
    let chain_input = OutPoint::new(Byte32::new([0xe6; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(753u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let victim = accept_remote(
        &mut authority,
        victim_tx.clone(),
        102,
        vec![chain_input.clone()],
        Capacity::shannons(1),
    );
    let victim_output = OutPoint::new(victim_tx.hash(), 0);
    let child_tx = TransactionBuilder::default()
        .version(754u32)
        .cell_dep(CellDep::new_builder().out_point(victim_output).build())
        .build();
    let child = accept_remote(
        &mut authority,
        child_tx,
        103,
        Vec::new(),
        Capacity::shannons(1),
    );

    let replacement_tx = TransactionBuilder::default()
        .version(755u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let replacement = admit(
        &mut authority,
        ValidatedAdmission::remote(replacement_tx, PeerIndex::from(104))
            .expect("replacement admission is valid"),
    );
    let replacement_settlement = verified_settlement_with_fee(
        checkout_continuous(&mut authority, &replacement),
        Vec::new(),
        vec![chain_input],
        Capacity::shannons(1_000_000),
    );
    apply_plan(
        authority
            .apply_settlement(replacement_settlement)
            .expect("replacement verifies"),
    );
    let replacement_version = owner_version(&authority, &replacement);
    let committed = authority
        .plan_accept_for_foundation(&replacement, replacement_version, AcceptedStatus::Pending)
        .expect("accepted dependency-reader closure plans")
        .apply();

    assert!(
        committed
            .removals
            .iter()
            .any(|removal| removal.hash == victim)
    );
    assert!(
        committed
            .removals
            .iter()
            .any(|removal| removal.hash == child)
    );
    assert!(authority.entry(&victim).is_none());
    assert!(authority.entry(&child).is_none());
    assert_eq!(drain_dependency_maintenance(&mut authority), 0);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_missing_growth_is_charged_or_becomes_budget_denied() {
    const COMPUTE_BYTES: usize = 4 * 1024;
    let retained = ResourceVector::new(2, 16 * 1024, 2, 1)
        .with_compute_capacity(COMPUTE_BYTES, 2)
        .expect("fixture compute partition fits");
    let constrained = ResourceLimits::with_residency_policy(
        retained,
        retained,
        retained,
        AcceptedResources::new(2, 16 * 1024, 16 * 1024, 16),
        ComputeLimits::new(COMPUTE_BYTES, COMPUTE_BYTES, 2),
        ResidencyPolicy::production(
            NonZeroUsize::new(64).expect("entry metadata is non-zero"),
            NonZeroUsize::new(2 * 1024).expect("edge metadata is non-zero"),
        ),
    )
    .expect("two-edge dependency fixture admits its indivisible grant");
    let mut authority = TxPoolAuthority::for_foundation(constrained);
    let declared = OutPoint::new(Byte32::new([0xf6; 32]), 0);
    let discovered = DependencyKey::Cell(OutPoint::new(Byte32::new([0xf7; 32]), 0));
    let transaction = input_transaction(760, declared);
    let hash = admit(
        &mut authority,
        ValidatedAdmission::remote(transaction, PeerIndex::from(97))
            .expect("one-edge admission is valid"),
    );
    let work = checkout_resolve(&mut authority, &hash);
    apply_plan(
        authority
            .apply_settlement(
                work.missing(vec![discovered])
                    .expect("byte-over-bound missing evidence yields an ordinary settlement"),
            )
            .expect("byte budget denial consumes the exact work capability"),
    );
    assert!(authority.entry(&hash).is_none());
    assert_eq!(authority.resources().preaccepted().edges, 0);
    assert_eq!(authority.resources().preaccepted().active_work, 0);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_duplicate_missing_receipt_cannot_bypass_the_work_grant() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let out_point = OutPoint::new(Byte32::new([0xf9; 32]), 0);
    let key = DependencyKey::Cell(out_point.clone());
    let hash = admit(
        &mut authority,
        ValidatedAdmission::remote(input_transaction(762, out_point), PeerIndex::from(106))
            .expect("duplicate-missing fixture admission is valid"),
    );
    let work = checkout_resolve(&mut authority, &hash);
    let over_grant = work
        .missing(vec![key; 17])
        .expect("over-grant evidence yields a typed settlement");
    apply_plan(
        authority
            .apply_settlement(over_grant)
            .expect("budget denial consumes the exact work capability"),
    );

    assert!(authority.entry(&hash).is_none());
    assert_eq!(authority.resources().preaccepted().active_work, 0);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_canonical_dependency_index_does_not_discount_ingress_edges() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let out_point = OutPoint::new(Byte32::new([0xf8; 32]), 0);
    let dependency_key = DependencyKey::Cell(out_point.clone());
    let duplicate_dep = CellDep::new_builder().out_point(out_point.clone()).build();
    let transaction = TransactionBuilder::default()
        .version(761u32)
        .input(CellInput::new(out_point, 0))
        .cell_dep(duplicate_dep.clone())
        .cell_dep(duplicate_dep)
        .build();
    let hash = admit(
        &mut authority,
        ValidatedAdmission::remote(transaction, PeerIndex::from(101))
            .expect("duplicate declarations remain a valid bounded admission"),
    );

    let Some(OwnedTx::PreAccepted(entry)) = authority.entry(&hash) else {
        panic!("admitted dependency fixture remains preaccepted");
    };
    assert_eq!(entry.dependencies().len(), 1);
    assert_eq!(entry.original_charge().edges, 3);
    assert_eq!(authority.resources().preaccepted().edges, 3);

    let work = checkout_resolve(&mut authority, &hash);
    apply_plan(
        authority
            .apply_settlement(
                work.missing(vec![dependency_key.clone()])
                    .expect("canonical missing evidence is bounded"),
            )
            .expect("duplicate declarations settle into one indexed wait"),
    );
    assert_eq!(authority.resources().preaccepted().edges, 3);
    apply_plan(
        authority
            .plan_dependency_availability_for_foundation(vec![dependency_key])
            .expect("availability plans")
            .expect("the canonical waiter is indexed"),
    );
    assert_eq!(drain_dependency_maintenance(&mut authority), 1);
    assert_eq!(authority.resources().preaccepted().edges, 3);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_retired_indexed_level_preserves_an_unindexed_race() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let dependency = OutPoint::new(Byte32::new([0xa8; 32]), 0);
    let key = DependencyKey::Cell(dependency.clone());
    let indexed = admit(
        &mut authority,
        ValidatedAdmission::remote(input_transaction(770, dependency), PeerIndex::from(98))
            .expect("indexed fixture admission is valid"),
    );
    let unrelated = OutPoint::new(Byte32::new([0xa9; 32]), 0);
    let resolving = admit(
        &mut authority,
        ValidatedAdmission::remote(input_transaction(771, unrelated), PeerIndex::from(99))
            .expect("unindexed fixture admission is valid"),
    );
    let work = checkout_resolve(&mut authority, &resolving);
    apply_plan(
        authority
            .plan_dependency_availability_for_foundation(vec![key.clone()])
            .expect("indexed availability plans")
            .expect("indexed dependency records a level"),
    );
    let indexed_version = owner_version(&authority, &indexed);
    apply_plan(
        authority
            .plan_terminalize_for_foundation(&indexed, indexed_version)
            .expect("removing the last indexed consumer plans"),
    );
    apply_plan(
        authority
            .apply_settlement(
                work.missing(vec![key])
                    .expect("new missing dependency is bounded"),
            )
            .expect("retired level still rejects the pre-event observation"),
    );
    assert!(matches!(
        authority.entry(&resolving),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_dirty_maintenance_cannot_outlive_its_last_charged_edge() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let declared = OutPoint::new(Byte32::new([0xba; 32]), 0);
    let discovered = DependencyKey::Cell(OutPoint::new(Byte32::new([0xbb; 32]), 0));
    let hash = admit(
        &mut authority,
        ValidatedAdmission::remote(input_transaction(780, declared), PeerIndex::from(100))
            .expect("dirty-edge fixture admission is valid"),
    );
    let work = checkout_resolve(&mut authority, &hash);
    apply_plan(
        authority
            .apply_settlement(
                work.missing(vec![discovered.clone()])
                    .expect("discovered dependency fits the grant"),
            )
            .expect("discovered dependency wait settles"),
    );
    apply_plan(
        authority
            .plan_dependency_availability_for_foundation(vec![discovered])
            .expect("discovered dependency availability plans")
            .expect("the exact waiter is indexed"),
    );
    assert_eq!(drain_dependency_maintenance(&mut authority), 1);
    assert!(
        authority
            .plan_dependency_maintenance()
            .expect("empty dirty frontier is valid")
            .is_none(),
        "removing the final expanded edge also removes its dirty traversal"
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_dependency_loss_work_counts_outputs_and_registered_origin_keys() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = TransactionBuilder::default()
        .version(790u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let actual = OutPoint::new(parent_tx.hash(), 0);
    let invalid_index = OutPoint::new(parent_tx.hash(), 9);
    let parent = admit(
        &mut authority,
        ValidatedAdmission::proposal(parent_tx).expect("parent admission is valid"),
    );
    for (version, dependency) in [(791, actual.clone()), (792, invalid_index.clone())] {
        let _child = admit(
            &mut authority,
            ValidatedAdmission::proposal(input_transaction(version, dependency))
                .expect("child admission is valid"),
        );
    }

    let work = authority
        .dependency_loss_work_for_foundation(std::slice::from_ref(&parent))
        .expect("loss-key construction is bounded");
    assert_eq!(work.output_keys, 3);
    assert_eq!(work.indexed_origin_keys, 2);
    assert_eq!(work.total(), Some(5));

    let parent_version = owner_version(&authority, &parent);
    apply_plan(
        authority
            .plan_terminalize_for_foundation(&parent, parent_version)
            .expect("parent terminalization publishes every exact origin key"),
    );
    let observed: BTreeSet<DependencyKey> = authority
        .drain_dependency_maintenance_for_foundation()
        .expect("loss-key maintenance consumes its rank-derived bound")
        .into_iter()
        .map(|step| step.key().clone())
        .collect();
    assert_eq!(
        observed,
        BTreeSet::from([
            DependencyKey::Cell(actual),
            DependencyKey::Cell(invalid_index),
        ])
    );
    assert_eq!(
        authority
            .dependency_maintenance_rank_for_foundation()
            .expect("the drained rank is representable")
            .value(),
        0
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_popular_dependency_maintenance_has_one_edge_steps_and_key_fairness() {
    const POPULAR_READERS: usize = 5;
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let popular_parent_tx = output_transaction(800);
    let sparse_parent_tx = output_transaction(801);
    let popular_outpoint = OutPoint::new(popular_parent_tx.hash(), 0);
    let sparse_outpoint = OutPoint::new(sparse_parent_tx.hash(), 0);
    let popular_key = DependencyKey::Cell(popular_outpoint.clone());
    let sparse_key = DependencyKey::Cell(sparse_outpoint.clone());
    let popular_parent = admit(
        &mut authority,
        ValidatedAdmission::proposal(popular_parent_tx).expect("popular parent admission is valid"),
    );
    let sparse_parent = admit(
        &mut authority,
        ValidatedAdmission::proposal(sparse_parent_tx).expect("sparse parent admission is valid"),
    );
    for offset in 0..POPULAR_READERS {
        let _child = admit(
            &mut authority,
            ValidatedAdmission::proposal(input_transaction(
                810 + offset as u32,
                popular_outpoint.clone(),
            ))
            .expect("popular child admission is valid"),
        );
    }
    let _sparse_child = admit(
        &mut authority,
        ValidatedAdmission::proposal(input_transaction(820, sparse_outpoint))
            .expect("sparse child admission is valid"),
    );

    for parent in [&popular_parent, &sparse_parent] {
        let version = owner_version(&authority, parent);
        apply_plan(
            authority
                .plan_terminalize_for_foundation(parent, version)
                .expect("parent terminalization publishes bounded loss"),
        );
    }

    let maintenance = authority
        .drain_dependency_maintenance_for_foundation()
        .expect("popular-key maintenance consumes its rank-derived bound");
    let mut order = Vec::new();
    let mut work_by_key = BTreeMap::<DependencyKey, (usize, usize)>::new();
    for step in maintenance {
        order.push((step.key().clone(), step.hash().is_some()));
        let work = work_by_key.entry(step.key().clone()).or_default();
        if step.hash().is_some() {
            work.0 = work.0.checked_add(1).expect("fixture edge count fits");
        } else {
            work.1 = work
                .1
                .checked_add(1)
                .expect("fixture completion count fits");
        }
    }
    assert_eq!(
        authority
            .dependency_maintenance_rank_for_foundation()
            .expect("the drained rank is representable")
            .value(),
        0
    );
    assert!(order.len() >= 2);
    assert!(order[0].1 && order[1].1);
    assert_ne!(order[0].0, order[1].0, "dirty keys advance round-robin");
    assert_eq!(work_by_key.get(&popular_key), Some(&(POPULAR_READERS, 1)));
    assert_eq!(work_by_key.get(&sparse_key), Some(&(1, 1)));
    assert_eq!(order.len(), POPULAR_READERS + 3);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_unindexed_false_retries_are_bounded_by_checked_out_work() {
    const ACTIVE_RESOLVERS: usize = 4;
    let attack_limits = ResourceLimits::new(
        ResourceVector::new(16, 256 * 1024, 256, 8),
        ResourceVector::new(12, 192 * 1024, 192, 6),
        ResourceVector::new(4, 64 * 1024, 64, 4),
        AcceptedResources::new(16, 256 * 1024, 256 * 1024, 256),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .expect("attack fixture admits every bounded active grant");
    let mut authority = TxPoolAuthority::for_foundation(attack_limits);
    let indexed_dependency = OutPoint::new(Byte32::new([0xc1; 32]), 0);
    let indexed_key = DependencyKey::Cell(indexed_dependency.clone());
    let indexed = admit(
        &mut authority,
        ValidatedAdmission::remote(
            input_transaction(830, indexed_dependency),
            PeerIndex::from(201),
        )
        .expect("indexed admission is valid"),
    );
    let mut active = Vec::new();
    for offset in 0..ACTIVE_RESOLVERS {
        let declared = OutPoint::new(Byte32::new([0xd0 + offset as u8; 32]), 0);
        let hash = admit(
            &mut authority,
            ValidatedAdmission::remote(
                input_transaction(831 + offset as u32, declared),
                PeerIndex::from(202 + offset),
            )
            .expect("resolver admission is valid"),
        );
        active.push(checkout_resolve(&mut authority, &hash));
    }
    apply_plan(
        authority
            .plan_dependency_availability_for_foundation(vec![indexed_key])
            .expect("indexed availability plans")
            .expect("the indexed consumer retains an exact level"),
    );
    let indexed_version = owner_version(&authority, &indexed);
    apply_plan(
        authority
            .plan_terminalize_for_foundation(&indexed, indexed_version)
            .expect("retiring the last exact level updates one constant watermark"),
    );

    let mut retries = 0usize;
    for (offset, work) in active.into_iter().enumerate() {
        let hash = TxIdentity::from_transaction(work.transaction()).raw;
        let discovered =
            DependencyKey::Cell(OutPoint::new(Byte32::new([0xe0 + offset as u8; 32]), 0));
        apply_plan(
            authority
                .apply_settlement(
                    work.missing(vec![discovered])
                        .expect("new dependency fits the active grant"),
                )
                .expect("the conservative retry settles atomically"),
        );
        if matches!(
            authority.entry(&hash),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
        ) {
            retries = retries.checked_add(1).expect("fixture retry count fits");
        }
    }
    assert_eq!(retries, ACTIVE_RESOLVERS);
    assert_eq!(authority.resources().preaccepted().active_work, 0);
    assert!(authority.primary_projection_consistent());
}
