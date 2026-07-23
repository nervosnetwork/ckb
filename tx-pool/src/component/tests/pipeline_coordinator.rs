use crate::component::pipeline_coordinator::{
    CoordinatorError, CoordinatorFeeGate, CoordinatorLimits, CoordinatorLocation,
    CoordinatorResidency, PayloadPhase, PipelineCoordinator, QueueKind, RawStage,
    TerminalDisposition,
};
use ckb_network::PeerIndex;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq)]
struct Raw(&'static str);

#[derive(Debug, PartialEq, Eq)]
struct Unverified(&'static str);

#[derive(Debug, PartialEq, Eq)]
struct Verified(&'static str);

fn hash(seed: u8) -> Byte32 {
    Byte32::new([seed; 32])
}

fn short(seed: u8) -> ProposalShortId {
    ProposalShortId::new([seed; 10])
}

fn set<const N: usize>(items: [Byte32; N]) -> HashSet<Byte32> {
    HashSet::from(items)
}

fn input(seed: u8) -> OutPoint {
    OutPoint::new(hash(seed), 0)
}

fn inputs<const N: usize>(items: [u8; N]) -> HashSet<OutPoint> {
    items.into_iter().map(input).collect()
}

fn roomy() -> PipelineCoordinator<Raw, Unverified, Verified> {
    PipelineCoordinator::new(CoordinatorLimits::new(
        CoordinatorResidency::new(100, 100_000),
        Some(CoordinatorResidency::new(20, 20_000)),
        16,
        16,
    ))
}

fn verify_candidate(
    coordinator: &mut PipelineCoordinator<Raw, Unverified, Verified>,
    seed: u8,
    conflict_inputs: HashSet<OutPoint>,
    fee: u64,
) -> Byte32 {
    let tx_hash = hash(seed);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(seed),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    let verify = coordinator.checkout_verify().unwrap().unwrap();
    let candidate = CoordinatorFeeGate::new(0, 0)
        .validate(tx_hash.clone(), conflict_inputs, fee, 100)
        .unwrap();
    coordinator
        .complete_verification_candidate(&verify, Verified("proof"), 30, candidate)
        .unwrap();
    tx_hash
}

#[test]
fn one_entry_and_revision_own_every_payload_phase_until_commit_handoff() {
    let mut coordinator = roomy();
    let tx_hash = hash(1);
    let peer: PeerIndex = 7.into();
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(1),
            Raw("raw"),
            RawStage::PreCheck,
            Some(peer),
            10,
            HashSet::new(),
        )
        .unwrap();
    coordinator.audit().unwrap();
    assert_eq!(coordinator.queue_len(QueueKind::PreCheck), 1);

    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    assert_eq!(*raw.payload, Raw("raw"));
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    let view = coordinator.view(&tx_hash).unwrap();
    assert_eq!(view.phase, PayloadPhase::Unverified);
    assert_eq!(view.location, CoordinatorLocation::VerifyQueued);
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 20));
    coordinator.audit().unwrap();

    let verify = coordinator.checkout_verify().unwrap().unwrap();
    assert_eq!(*verify.payload, Unverified("resolved"));
    coordinator
        .complete_verification(&verify, Verified("proof"), 30)
        .unwrap();
    let commit = coordinator.begin_next_commit().unwrap().unwrap();
    assert_eq!(*commit.payload, Verified("proof"));
    coordinator.audit().unwrap();

    let handoff = coordinator.commit_handoff(&commit).unwrap();
    assert_eq!(handoff.hash, tx_hash);
    assert_eq!(*handoff.raw, Raw("raw"));
    assert_eq!(*handoff.verified, Verified("proof"));
    assert_eq!(handoff.peer, Some(peer));
    assert!(coordinator.is_empty());
    assert_eq!(coordinator.usage(), CoordinatorResidency::default());
    coordinator.audit().unwrap();
}

#[test]
fn administrative_terminal_api_cannot_express_commit_and_releases_all_indexes() {
    let mut coordinator = roomy();
    let tx_hash = hash(2);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(2),
            Raw("raw"),
            RawStage::Resolve,
            None,
            10,
            set([hash(20)]),
        )
        .unwrap();

    let terminal = coordinator
        .force_terminalize(&tx_hash, TerminalDisposition::Removed)
        .unwrap()
        .unwrap();
    assert_eq!(terminal.disposition, TerminalDisposition::Removed);
    assert_eq!(*terminal.raw, Raw("raw"));
    assert!(terminal.later_phase.is_none());
    assert!(coordinator.hash_by_short_id(&short(2)).is_none());
    assert!(coordinator.is_empty());
    coordinator.audit().unwrap();
}

#[test]
fn parent_invalidation_demotes_payload_and_makes_active_verify_lease_stale() {
    let mut coordinator = roomy();
    let tx_hash = hash(3);
    let parent = hash(30);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(3),
            Raw("raw"),
            RawStage::Resolve,
            None,
            10,
            set([parent.clone()]),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::Resolve)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 50)
        .unwrap();
    let verify = coordinator.checkout_verify().unwrap().unwrap();

    assert_eq!(
        coordinator.parent_unavailable(&parent).unwrap(),
        vec![tx_hash.clone()]
    );
    let view = coordinator.view(&tx_hash).unwrap();
    assert_eq!(view.phase, PayloadPhase::Raw);
    assert_eq!(view.charge_bytes, 10);
    assert_eq!(
        view.location,
        CoordinatorLocation::WaitingParents {
            missing: set([parent.clone()])
        }
    );
    assert!(matches!(
        coordinator.complete_verification(&verify, Verified("stale"), 60),
        Err(CoordinatorError::RevisionMismatch { .. })
    ));

    let ready = coordinator.parent_available(&parent).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(coordinator.queue_len(QueueKind::Resolve), 1);
    coordinator.audit().unwrap();
}

#[test]
fn final_parent_wake_is_exactly_once_and_batch_preflight_is_atomic() {
    let mut coordinator = roomy();
    let parent = hash(40);
    let child_a = hash(4);
    let child_b = hash(5);
    for (child, seed) in [(child_a.clone(), 4), (child_b.clone(), 5)] {
        coordinator
            .admit_raw(
                child.clone(),
                short(seed),
                Raw("raw"),
                RawStage::Resolve,
                None,
                10,
                set([parent.clone()]),
            )
            .unwrap();
        let lease = coordinator
            .checkout_raw(RawStage::Resolve)
            .unwrap()
            .unwrap();
        coordinator
            .wait_for_parents(&lease, set([parent.clone()]))
            .unwrap();
    }
    coordinator
        .set_revision_for_test(&child_b, u64::MAX)
        .unwrap();

    assert_eq!(
        coordinator.parent_available(&parent),
        Err(CoordinatorError::RevisionExhausted(child_b.clone()))
    );
    for child in [&child_a, &child_b] {
        assert!(matches!(
            coordinator.view(child).unwrap().location,
            CoordinatorLocation::WaitingParents { ref missing }
                if missing == &set([parent.clone()])
        ));
    }
    assert_eq!(coordinator.queue_len(QueueKind::Resolve), 0);
    coordinator.audit().unwrap();
}

#[test]
fn revision_exhaustion_does_not_consume_the_only_live_queue_ticket() {
    let mut coordinator = roomy();
    let tx_hash = hash(6);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(6),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    coordinator
        .set_revision_for_test(&tx_hash, u64::MAX)
        .unwrap();

    assert!(matches!(
        coordinator.checkout_raw(RawStage::PreCheck),
        Err(CoordinatorError::RevisionExhausted(hash)) if hash == tx_hash
    ));
    assert_eq!(coordinator.queue_len(QueueKind::PreCheck), 1);
    assert!(coordinator.physical_queue_slots_for_test(QueueKind::PreCheck) >= 1);
    coordinator.audit().unwrap();
}

#[test]
fn removed_and_readmitted_hash_rejects_the_old_worker_incarnation() {
    let mut coordinator = roomy();
    let tx_hash = hash(7);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(7),
            Raw("old"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let old = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .force_terminalize(&tx_hash, TerminalDisposition::Cleared)
        .unwrap();
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(7),
            Raw("new"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();

    assert!(matches!(
        coordinator.complete_raw(&old, Unverified("stale"), 20),
        Err(CoordinatorError::IncarnationMismatch { .. })
    ));
    let current = coordinator.view(&tx_hash).unwrap();
    assert_eq!(current.phase, PayloadPhase::Raw);
    assert_eq!(
        current.location,
        CoordinatorLocation::RawQueued(RawStage::PreCheck)
    );
    coordinator.audit().unwrap();
}

#[test]
fn identity_budget_and_fanout_failures_do_not_partially_admit() {
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(CoordinatorLimits::new(
            CoordinatorResidency::new(2, 20),
            Some(CoordinatorResidency::new(1, 10)),
            1,
            1,
        ));
    let peer: PeerIndex = 1.into();
    let parent = hash(80);
    let first = hash(8);
    coordinator
        .admit_raw(
            first.clone(),
            short(8),
            Raw("first"),
            RawStage::Resolve,
            Some(peer),
            10,
            set([parent.clone()]),
        )
        .unwrap();

    assert!(matches!(
        coordinator.admit_raw(
            hash(9),
            short(9),
            Raw("fanout"),
            RawStage::Resolve,
            None,
            10,
            set([parent.clone()]),
        ),
        Err(CoordinatorError::ParentFanoutLimitExceeded(hash)) if hash == parent
    ));
    assert_eq!(
        coordinator.admit_raw(
            hash(10),
            short(8),
            Raw("collision"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        ),
        Err(CoordinatorError::ShortIdCollision {
            short_id: short(8),
            existing_hash: first,
        })
    );
    assert_eq!(coordinator.len(), 1);
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 10));
    coordinator.audit().unwrap();
}

#[test]
fn failed_phase_recharge_leaves_payload_location_and_queue_unchanged() {
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> = PipelineCoordinator::new(
        CoordinatorLimits::new(CoordinatorResidency::new(1, 15), None, 4, 4),
    );
    let tx_hash = hash(11);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(11),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let lease = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();

    assert_eq!(
        coordinator.complete_raw(&lease, Unverified("too large"), 16),
        Err(CoordinatorError::GlobalBudgetExceeded)
    );
    let view = coordinator.view(&tx_hash).unwrap();
    assert_eq!(view.phase, PayloadPhase::Raw);
    assert_eq!(
        view.location,
        CoordinatorLocation::RawActive(RawStage::PreCheck)
    );
    assert_eq!(view.charge_bytes, 10);
    assert_eq!(coordinator.queue_len(QueueKind::Verify), 0);
    coordinator.audit().unwrap();
}

#[test]
fn abort_commit_requeues_once_and_makes_the_old_commit_lease_stale() {
    let mut coordinator = roomy();
    let tx_hash = hash(12);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(12),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    let verify = coordinator.checkout_verify().unwrap().unwrap();
    coordinator
        .complete_verification(&verify, Verified("proof"), 30)
        .unwrap();
    let old_commit = coordinator.begin_next_commit().unwrap().unwrap();
    coordinator.abort_commit(&old_commit).unwrap();

    assert!(matches!(
        coordinator.commit_handoff(&old_commit),
        Err(CoordinatorError::RevisionMismatch { .. })
    ));
    assert_eq!(coordinator.queue_len(QueueKind::Commit), 1);
    let new_commit = coordinator.begin_next_commit().unwrap().unwrap();
    assert_ne!(old_commit.version, new_commit.version);
    coordinator.audit().unwrap();
}

#[test]
fn unverified_high_fee_work_cannot_own_or_preempt_a_conflict_domain() {
    let mut coordinator = roomy();
    let contested = input(1);
    let verified = verify_candidate(
        &mut coordinator,
        20,
        HashSet::from([contested.clone()]),
        100,
    );
    assert_eq!(
        coordinator.active_conflict_owner(&contested),
        Some(&verified)
    );

    let unverified_hash = hash(21);
    coordinator
        .admit_raw(
            unverified_hash.clone(),
            short(21),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("unverified high fee"), 20)
        .unwrap();

    assert_eq!(
        coordinator.active_conflict_owner(&contested),
        Some(&verified)
    );
    assert_eq!(
        coordinator.view(&unverified_hash).unwrap().location,
        CoordinatorLocation::VerifyQueued
    );
    coordinator.audit().unwrap();
}

#[test]
fn under_fee_candidate_cannot_become_verified_conflict_state() {
    let mut coordinator = roomy();
    let contested = input(9);
    let owner = verify_candidate(
        &mut coordinator,
        33,
        HashSet::from([contested.clone()]),
        1_000,
    );
    let candidate_hash = hash(34);
    coordinator
        .admit_raw(
            candidate_hash.clone(),
            short(34),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    let _verify = coordinator.checkout_verify().unwrap().unwrap();

    assert_eq!(
        CoordinatorFeeGate::new(2_000, 0).validate(
            candidate_hash.clone(),
            HashSet::from([contested.clone()]),
            1_999,
            100,
        ),
        Err(CoordinatorError::UnderReplacementFee {
            hash: candidate_hash.clone(),
            required: 2_000,
            actual: 1_999,
        })
    );
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&owner));
    assert_eq!(
        coordinator.view(&candidate_hash).unwrap().phase,
        PayloadPhase::Unverified
    );
    assert_eq!(coordinator.conflict_edge_count(), 1);
    coordinator.audit().unwrap();
}

#[test]
fn higher_verified_candidate_preempts_and_removal_rechecks_the_loser() {
    let mut coordinator = roomy();
    let contested = input(2);
    let low = verify_candidate(
        &mut coordinator,
        22,
        HashSet::from([contested.clone()]),
        100,
    );
    let high = verify_candidate(
        &mut coordinator,
        23,
        HashSet::from([contested.clone()]),
        200,
    );

    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&high));
    assert!(matches!(
        coordinator.view(&low).unwrap().location,
        CoordinatorLocation::WaitingConflict { ref blockers }
            if blockers == &HashSet::from([high.clone()])
    ));
    coordinator.audit().unwrap();

    coordinator
        .force_terminalize(&high, TerminalDisposition::Rejected)
        .unwrap();
    assert_eq!(
        coordinator.view(&low).unwrap().location,
        CoordinatorLocation::ConflictRecheck
    );
    assert_eq!(coordinator.conflict_recheck_len(), 1);
    let activated = coordinator.drain_conflict_rechecks(1).unwrap();
    assert_eq!(activated.len(), 1);
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&low));
    assert_eq!(
        coordinator.view(&low).unwrap().location,
        CoordinatorLocation::ReadyToCommit
    );
    coordinator.audit().unwrap();
}

#[test]
fn multi_input_verified_candidate_is_all_or_none_and_committing_is_frozen() {
    let mut coordinator = roomy();
    let left_input = input(3);
    let right_input = input(4);
    let left = verify_candidate(
        &mut coordinator,
        24,
        HashSet::from([left_input.clone()]),
        100,
    );
    let right = verify_candidate(
        &mut coordinator,
        25,
        HashSet::from([right_input.clone()]),
        100,
    );
    let both = verify_candidate(
        &mut coordinator,
        26,
        HashSet::from([left_input.clone(), right_input.clone()]),
        200,
    );

    assert_eq!(coordinator.active_conflict_owner(&left_input), Some(&both));
    assert_eq!(coordinator.active_conflict_owner(&right_input), Some(&both));
    for loser in [&left, &right] {
        assert!(matches!(
            coordinator.view(loser).unwrap().location,
            CoordinatorLocation::WaitingConflict { ref blockers }
                if blockers == &HashSet::from([both.clone()])
        ));
    }

    let committing = coordinator.begin_next_commit().unwrap().unwrap();
    assert_eq!(committing.hash, both);
    let later = verify_candidate(
        &mut coordinator,
        27,
        HashSet::from([left_input.clone(), right_input.clone()]),
        300,
    );
    assert!(matches!(
        coordinator.view(&later).unwrap().location,
        CoordinatorLocation::WaitingConflict { ref blockers }
            if blockers == &HashSet::from([committing.hash.clone()])
    ));
    assert_eq!(
        coordinator.active_conflict_owner(&left_input),
        Some(&committing.hash)
    );
    assert_eq!(
        coordinator.active_conflict_owner(&right_input),
        Some(&committing.hash)
    );
    coordinator.audit().unwrap();
}

#[test]
fn preempted_blockers_move_their_old_waiters_to_bounded_recheck_work() {
    let mut coordinator = roomy();
    let contested = input(5);
    let middle = verify_candidate(
        &mut coordinator,
        28,
        HashSet::from([contested.clone()]),
        200,
    );
    let low = verify_candidate(
        &mut coordinator,
        29,
        HashSet::from([contested.clone()]),
        100,
    );
    assert!(matches!(
        coordinator.view(&low).unwrap().location,
        CoordinatorLocation::WaitingConflict { .. }
    ));

    let high = verify_candidate(
        &mut coordinator,
        30,
        HashSet::from([contested.clone()]),
        300,
    );
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&high));
    assert!(matches!(
        coordinator.view(&middle).unwrap().location,
        CoordinatorLocation::WaitingConflict { .. }
    ));
    assert_eq!(
        coordinator.view(&low).unwrap().location,
        CoordinatorLocation::ConflictRecheck
    );
    assert_eq!(coordinator.conflict_recheck_len(), 1);
    coordinator.audit().unwrap();

    assert!(coordinator.drain_conflict_rechecks(1).unwrap().is_empty());
    assert!(matches!(
        coordinator.view(&low).unwrap().location,
        CoordinatorLocation::WaitingConflict { ref blockers }
            if blockers == &HashSet::from([high])
    ));
    coordinator.audit().unwrap();
}

#[test]
fn conflict_limits_fail_before_verified_state_or_indexes_change() {
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> = PipelineCoordinator::new(
        CoordinatorLimits::new(CoordinatorResidency::new(10, 1_000), None, 4, 4)
            .with_conflict_limits(1, 1, 1),
    );
    let first = verify_candidate(&mut coordinator, 31, inputs([6]), 100);
    assert_eq!(coordinator.conflict_edge_count(), 1);

    let second_hash = hash(32);
    coordinator
        .admit_raw(
            second_hash.clone(),
            short(32),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    let verify = coordinator.checkout_verify().unwrap().unwrap();
    let candidate = CoordinatorFeeGate::new(0, 0)
        .validate(second_hash.clone(), inputs([6]), 200, 100)
        .unwrap();
    assert!(matches!(
        coordinator.complete_verification_candidate(&verify, Verified("proof"), 30, candidate),
        Err(CoordinatorError::ConflictEdgeLimitExceeded)
            | Err(CoordinatorError::ConflictCandidateLimitExceeded(_))
    ));

    let view = coordinator.view(&second_hash).unwrap();
    assert_eq!(view.phase, PayloadPhase::Unverified);
    assert_eq!(view.location, CoordinatorLocation::VerifyActive);
    assert_eq!(coordinator.active_conflict_owner(&input(6)), Some(&first));
    assert_eq!(coordinator.conflict_edge_count(), 1);
    coordinator.audit().unwrap();
}

#[test]
fn waiter_revision_exhaustion_cannot_half_remove_its_active_blocker() {
    let mut coordinator = roomy();
    let contested = input(10);
    let owner = verify_candidate(
        &mut coordinator,
        35,
        HashSet::from([contested.clone()]),
        200,
    );
    let waiter = verify_candidate(
        &mut coordinator,
        36,
        HashSet::from([contested.clone()]),
        100,
    );
    coordinator
        .set_revision_for_test(&waiter, u64::MAX)
        .unwrap();

    assert!(matches!(
        coordinator.force_terminalize(&owner, TerminalDisposition::Rejected),
        Err(CoordinatorError::RevisionExhausted(hash)) if hash == waiter
    ));
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&owner));
    assert_eq!(
        coordinator.view(&owner).unwrap().location,
        CoordinatorLocation::ReadyToCommit
    );
    assert!(matches!(
        coordinator.view(&waiter).unwrap().location,
        CoordinatorLocation::WaitingConflict { ref blockers }
            if blockers == &HashSet::from([owner])
    ));
    assert_eq!(coordinator.queue_len(QueueKind::Commit), 1);
    coordinator.audit().unwrap();
}

#[test]
fn successful_candidate_handoff_rejects_current_direct_cohort_only() {
    let mut coordinator = roomy();
    let contested = input(11);
    let independent_input = input(12);
    let winner = verify_candidate(
        &mut coordinator,
        37,
        HashSet::from([contested.clone()]),
        300,
    );
    let loser = verify_candidate(
        &mut coordinator,
        38,
        HashSet::from([contested.clone()]),
        100,
    );
    let independent = verify_candidate(
        &mut coordinator,
        39,
        HashSet::from([independent_input.clone()]),
        50,
    );
    let committing = coordinator.begin_next_commit().unwrap().unwrap();
    assert_eq!(committing.hash, winner);
    let late_loser = verify_candidate(
        &mut coordinator,
        40,
        HashSet::from([contested.clone()]),
        400,
    );
    assert!(matches!(
        coordinator.view(&late_loser).unwrap().location,
        CoordinatorLocation::WaitingConflict { .. }
    ));
    assert!(matches!(
        coordinator.commit_handoff(&committing),
        Err(CoordinatorError::ConflictInvariant)
    ));

    let handoff = coordinator.commit_candidate_handoff(&committing).unwrap();
    assert_eq!(handoff.winner.hash, winner);
    let rejected: HashSet<_> = handoff
        .rejected
        .into_iter()
        .map(|record| {
            assert_eq!(record.disposition, TerminalDisposition::Rejected);
            record.hash
        })
        .collect();
    assert_eq!(rejected, HashSet::from([loser, late_loser]));
    assert!(coordinator.view(&independent).is_some());
    assert_eq!(
        coordinator.active_conflict_owner(&independent_input),
        Some(&independent)
    );
    assert_eq!(coordinator.conflict_edge_count(), 1);
    coordinator.audit().unwrap();
}

#[test]
fn clear_is_one_batch_and_does_not_revise_conflict_waiters() {
    let mut coordinator = roomy();
    let contested = input(13);
    let _owner = verify_candidate(
        &mut coordinator,
        41,
        HashSet::from([contested.clone()]),
        200,
    );
    let waiter = verify_candidate(&mut coordinator, 42, HashSet::from([contested]), 100);
    coordinator
        .set_revision_for_test(&waiter, u64::MAX)
        .unwrap();

    let cleared = coordinator.clear().unwrap();
    assert_eq!(cleared.len(), 2);
    assert!(
        cleared
            .iter()
            .all(|record| record.disposition == TerminalDisposition::Cleared)
    );
    assert!(coordinator.is_empty());
    assert_eq!(coordinator.usage(), CoordinatorResidency::default());
    assert_eq!(coordinator.conflict_edge_count(), 0);
    assert_eq!(coordinator.conflict_recheck_len(), 0);
    coordinator.audit().unwrap();
}
