use crate::component::conflict_scheduler::{
    ConflictError, ConflictLimits, ConflictScheduler, ConflictState, ReplacementFeeGate,
};
use ckb_types::packed::{Byte32, OutPoint};
use std::collections::HashSet;

fn hash(seed: u8) -> Byte32 {
    Byte32::new([seed; 32])
}

fn input(seed: u8) -> OutPoint {
    OutPoint::new(hash(seed), 0)
}

fn inputs<const N: usize>(seeds: [u8; N]) -> HashSet<OutPoint> {
    seeds.into_iter().map(input).collect()
}

fn eligible(
    seed: u8,
    input_seeds: &[u8],
    fee: u64,
) -> super::super::conflict_scheduler::EligibleCandidate {
    ReplacementFeeGate::new(0, 0)
        .validate(
            hash(seed),
            input_seeds.iter().copied().map(input).collect(),
            fee,
            100,
        )
        .unwrap()
}

fn roomy_scheduler() -> ConflictScheduler {
    ConflictScheduler::new(ConflictLimits::new(100, 1_000, 100))
}

fn only_active(
    changes: crate::component::conflict_scheduler::ConflictChanges,
) -> crate::component::conflict_scheduler::ConflictTicket {
    assert!(changes.preempted.is_empty());
    assert_eq!(changes.activated.len(), 1);
    changes.activated.into_iter().next().unwrap()
}

#[test]
fn under_fee_candidate_cannot_become_held_or_scheduled() {
    let mut scheduler = roomy_scheduler();
    let tx_hash = hash(1);
    let gate = ReplacementFeeGate::new(100, 1_000);

    assert!(matches!(
        gate.validate(tx_hash.clone(), inputs([1]), 99, 100),
        Err(ConflictError::UnderReplacementFee {
            required: 100,
            actual: 99,
            ..
        })
    ));
    assert!(matches!(
        ReplacementFeeGate::new(0, 2_000).validate(tx_hash, inputs([1]), 100, 100),
        Err(ConflictError::UnderFeeRate {
            required_per_kb: 2_000,
            ..
        })
    ));
    assert_eq!(scheduler.len(), 0);

    let admitted = gate.validate(hash(2), inputs([1]), 100, 100).unwrap();
    let ticket = only_active(scheduler.register(admitted).unwrap());
    assert_eq!(ticket.hash, hash(2));
    scheduler.audit().unwrap();
}

#[test]
fn higher_fee_preempts_and_abort_restores_best_waiter() {
    let mut scheduler = roomy_scheduler();
    let low = only_active(scheduler.register(eligible(10, &[1], 100)).unwrap());
    let medium_changes = scheduler.register(eligible(11, &[1], 200)).unwrap();
    assert_eq!(medium_changes.preempted, vec![low.hash.clone()]);
    let medium = medium_changes.activated[0].clone();
    let high_changes = scheduler.register(eligible(12, &[1], 300)).unwrap();
    assert_eq!(high_changes.preempted, vec![medium.hash.clone()]);
    let high = high_changes.activated[0].clone();

    assert!(matches!(
        scheduler.view(&low.hash).unwrap().state,
        ConflictState::Waiting { .. }
    ));
    assert!(matches!(
        scheduler.view(&medium.hash).unwrap().state,
        ConflictState::Waiting { .. }
    ));
    assert_eq!(scheduler.active_owner(&input(1)), Some(&high.hash));

    let restored = scheduler.abort_active(&high).unwrap();
    assert_eq!(restored.activated.len(), 1);
    assert_eq!(restored.activated[0].hash, medium.hash);
    assert_eq!(scheduler.active_owner(&input(1)), Some(&medium.hash));
    scheduler.audit().unwrap();
}

#[test]
fn multi_input_candidate_never_partially_preempts() {
    let mut scheduler = roomy_scheduler();
    let low = only_active(scheduler.register(eligible(20, &[1], 100)).unwrap());
    let high = only_active(scheduler.register(eligible(21, &[2], 300)).unwrap());
    let middle_changes = scheduler.register(eligible(22, &[1, 2], 200)).unwrap();

    assert!(middle_changes.activated.is_empty());
    assert!(middle_changes.preempted.is_empty());
    assert_eq!(scheduler.active_owner(&input(1)), Some(&low.hash));
    assert_eq!(scheduler.active_owner(&input(2)), Some(&high.hash));
    assert!(matches!(
        scheduler.view(&hash(22)).unwrap().state,
        ConflictState::Waiting { ref blockers }
            if blockers == &HashSet::from([low.hash.clone(), high.hash.clone()])
    ));

    let changes = scheduler.abort_active(&high).unwrap();
    assert_eq!(changes.preempted, vec![low.hash.clone()]);
    assert_eq!(changes.activated.len(), 1);
    assert_eq!(changes.activated[0].hash, hash(22));
    assert_eq!(scheduler.active_owner(&input(1)), Some(&hash(22)));
    assert_eq!(scheduler.active_owner(&input(2)), Some(&hash(22)));
    scheduler.audit().unwrap();
}

#[test]
fn replacement_does_not_disturb_independent_conflict_domain() {
    let mut scheduler = roomy_scheduler();
    let first = only_active(scheduler.register(eligible(30, &[1], 100)).unwrap());
    let independent = only_active(scheduler.register(eligible(31, &[2], 50)).unwrap());
    let replacement = scheduler.register(eligible(32, &[1], 200)).unwrap();

    assert_eq!(replacement.preempted, vec![first.hash]);
    assert_eq!(replacement.activated[0].hash, hash(32));
    assert_eq!(scheduler.active_owner(&input(2)), Some(&independent.hash));
    assert_eq!(
        scheduler.view(&independent.hash).unwrap().state,
        ConflictState::Active
    );
    scheduler.audit().unwrap();
}

#[test]
fn committing_candidate_is_not_preemptable() {
    let mut scheduler = roomy_scheduler();
    let active = only_active(scheduler.register(eligible(40, &[1], 100)).unwrap());
    let committing = scheduler.begin_commit(&active).unwrap();
    let later = scheduler.register(eligible(41, &[1], 1_000)).unwrap();

    assert!(later.activated.is_empty());
    assert!(later.preempted.is_empty());
    assert_eq!(
        scheduler.view(&committing.hash).unwrap().state,
        ConflictState::Committing
    );
    assert!(matches!(
        scheduler.view(&hash(41)).unwrap().state,
        ConflictState::Waiting { .. }
    ));

    let changes = scheduler.abort_commit(&committing).unwrap();
    assert_eq!(changes.activated.len(), 1);
    assert_eq!(changes.activated[0].hash, hash(41));
    scheduler.audit().unwrap();
}

#[test]
fn successful_commit_rejects_only_direct_conflicts() {
    let mut scheduler = roomy_scheduler();
    let victim = only_active(scheduler.register(eligible(50, &[1], 100)).unwrap());
    let independent = only_active(scheduler.register(eligible(51, &[2], 100)).unwrap());
    let winner_changes = scheduler.register(eligible(52, &[1], 200)).unwrap();
    assert_eq!(winner_changes.preempted, vec![victim.hash.clone()]);
    let winner = winner_changes.activated[0].clone();
    let committing = scheduler.begin_commit(&winner).unwrap();
    let outcome = scheduler.commit_succeeded(&committing).unwrap();

    assert_eq!(outcome.winner, winner.hash);
    assert_eq!(outcome.rejected, vec![victim.hash]);
    assert!(scheduler.view(&hash(52)).is_none());
    assert!(scheduler.view(&hash(50)).is_none());
    assert_eq!(
        scheduler.view(&independent.hash).unwrap().state,
        ConflictState::Active
    );
    assert_eq!(scheduler.active_owner(&input(2)), Some(&independent.hash));
    scheduler.audit().unwrap();
}

#[test]
fn stale_preempted_ticket_cannot_start_commit() {
    let mut scheduler = roomy_scheduler();
    let stale = only_active(scheduler.register(eligible(60, &[1], 100)).unwrap());
    scheduler.register(eligible(61, &[1], 200)).unwrap();

    assert!(matches!(
        scheduler.begin_commit(&stale),
        Err(ConflictError::StaleTicket { .. })
    ));
    assert!(matches!(
        scheduler.abort_active(&stale),
        Err(ConflictError::StaleTicket { .. })
    ));
    scheduler.audit().unwrap();
}

#[test]
fn exact_fee_tie_keeps_earlier_candidate_active() {
    let mut scheduler = roomy_scheduler();
    let earlier = only_active(scheduler.register(eligible(70, &[1], 100)).unwrap());
    let later = scheduler.register(eligible(71, &[1], 100)).unwrap();

    assert!(later.activated.is_empty());
    assert!(later.preempted.is_empty());
    assert_eq!(scheduler.active_owner(&input(1)), Some(&earlier.hash));
    scheduler.audit().unwrap();
}

#[test]
fn candidate_and_edge_limits_fail_without_partial_indexes() {
    let mut scheduler = ConflictScheduler::new(ConflictLimits::new(2, 2, 1));
    scheduler.register(eligible(80, &[1], 100)).unwrap();
    scheduler.register(eligible(81, &[2], 100)).unwrap();
    assert_eq!(
        scheduler.register(eligible(82, &[3], 100)).unwrap_err(),
        ConflictError::CandidateLimitExceeded
    );
    assert_eq!(scheduler.len(), 2);
    assert_eq!(scheduler.edge_count(), 2);
    assert!(scheduler.view(&hash(82)).is_none());
    scheduler.audit().unwrap();

    let mut edge_limited = ConflictScheduler::new(ConflictLimits::new(10, 2, 2));
    edge_limited.register(eligible(83, &[4, 5], 100)).unwrap();
    assert_eq!(
        edge_limited.register(eligible(84, &[6], 100)).unwrap_err(),
        ConflictError::EdgeLimitExceeded
    );
    assert_eq!(edge_limited.edge_count(), 2);
    edge_limited.audit().unwrap();
}

#[test]
fn preempted_waiters_on_freed_inputs_rebalance_independently() {
    let mut scheduler = roomy_scheduler();
    let original = only_active(scheduler.register(eligible(90, &[1, 2], 100)).unwrap());
    scheduler.register(eligible(91, &[1], 50)).unwrap();
    scheduler.register(eligible(92, &[2], 60)).unwrap();

    let replacement = scheduler.register(eligible(93, &[1], 200)).unwrap();
    assert_eq!(replacement.preempted, vec![original.hash.clone()]);
    assert_eq!(scheduler.active_owner(&input(1)), Some(&hash(93)));
    // The candidate that only conflicts on input 2 no longer has a blocker.
    assert_eq!(scheduler.active_owner(&input(2)), Some(&hash(92)));
    assert_eq!(
        scheduler.view(&hash(92)).unwrap().state,
        ConflictState::Active
    );
    assert!(matches!(
        scheduler.view(&hash(91)).unwrap().state,
        ConflictState::Waiting { .. }
    ));
    assert!(matches!(
        scheduler.view(&original.hash).unwrap().state,
        ConflictState::Waiting { .. }
    ));
    scheduler.audit().unwrap();
}

#[test]
fn clear_releases_every_conflict_index() {
    let mut scheduler = roomy_scheduler();
    scheduler.register(eligible(100, &[1], 100)).unwrap();
    scheduler.register(eligible(101, &[1], 200)).unwrap();
    scheduler.register(eligible(102, &[2], 100)).unwrap();
    scheduler.clear();

    assert_eq!(scheduler.len(), 0);
    assert_eq!(scheduler.edge_count(), 0);
    assert!(scheduler.active_owner(&input(1)).is_none());
    assert!(scheduler.active_owner(&input(2)).is_none());
    scheduler.audit().unwrap();
}
