use super::super::ban::{PeerBanError, PeerBanSlotBank};
use crate::constants::MALFORMED_TX_BAN_SECONDS;
use ckb_network::PeerIndex;
use std::time::{Duration, Instant};

#[test]
fn live_marker_is_never_shortened_and_expired_markers_are_pruned_on_record() {
    let registry = PeerBanSlotBank::default();
    let start = Instant::now();
    let first = PeerIndex::from(1);
    let second = PeerIndex::from(2);

    let long = registry
        .plan_record(first, start)
        .expect("one marker reserves");
    let almost_expired = Duration::from_secs(MALFORMED_TX_BAN_SECONDS - 1);
    assert_eq!(
        long.lease().remaining_at(start + almost_expired),
        Some(Duration::from_secs(1)),
        "the external ban consumes the same fixed deadline"
    );
    long.begin().expect("the reserved slot begins").finish();
    let short = registry
        .plan_record(first, start + Duration::from_secs(1))
        .expect("replacement needs no allocation");
    short.begin().expect("the existing slot begins").finish();
    assert_eq!(
        registry.snapshot().len(),
        1,
        "a repeated live ban has no duplicate expiration owner"
    );
    assert!(registry.contains_at(first, start + almost_expired));

    let next = registry
        .plan_record(
            second,
            start + Duration::from_secs(MALFORMED_TX_BAN_SECONDS + 1),
        )
        .expect("expired marker capacity is reusable");
    next.begin().expect("the replacement slot begins").finish();
    let after_expiry = start + Duration::from_secs(MALFORMED_TX_BAN_SECONDS + 1);
    assert!(!registry.contains_at(first, after_expiry));
    assert!(registry.contains_at(second, after_expiry));
    assert_eq!(registry.snapshot().len(), 1);
    assert!(registry.semantically_consistent());
}

#[test]
fn saturation_evicts_the_oldest_fence_and_keeps_a_hard_bound() {
    let registry = PeerBanSlotBank::with_limit_for_test(2);
    let start = Instant::now();
    let first = PeerIndex::from(1);
    let second = PeerIndex::from(2);
    let third = PeerIndex::from(3);

    for (offset, peer) in [(0, first), (1, second), (2, third)] {
        let delta = registry
            .plan_record(peer, start + Duration::from_secs(offset))
            .expect("the fixed-capacity fixture reuses an owned slot");
        delta.begin().expect("the staged slot begins").finish();
        assert!(registry.semantically_consistent());
    }

    assert!(!registry.contains_at(first, start + Duration::from_secs(3)));
    assert!(registry.contains_at(second, start + Duration::from_secs(3)));
    assert!(registry.contains_at(third, start + Duration::from_secs(3)));
    assert_eq!(registry.snapshot().len(), 2);
}

#[test]
fn full_bank_never_hides_the_oldest_behind_a_live_repeat_stage() {
    let registry = PeerBanSlotBank::with_limit_for_test(2);
    let start = Instant::now();
    let oldest = PeerIndex::from(10);
    let newer = PeerIndex::from(11);
    let newcomer = PeerIndex::from(12);
    for (offset, peer) in [(0, oldest), (1, newer)] {
        registry
            .plan_record(peer, start + Duration::from_secs(offset))
            .expect("the fixture fills one exact slot")
            .begin()
            .expect("the exact reservation begins")
            .finish();
    }

    let repeat = registry
        .plan_record(oldest, start + Duration::from_secs(2))
        .expect("a live repeat stages without changing its lease or order");
    assert!(matches!(
        registry.plan_record(newcomer, start + Duration::from_secs(3)),
        Err(PeerBanError::Contention)
    ));
    drop(repeat);

    let repeat = registry
        .plan_record(oldest, start + Duration::from_secs(4))
        .expect("the live repeat stages again");
    let repeat = repeat.begin().expect("the live repeat begins");
    assert!(matches!(
        registry.plan_record(newcomer, start + Duration::from_secs(5)),
        Err(PeerBanError::Contention)
    ));
    repeat.finish();

    let replacement = registry
        .plan_record(newcomer, start + Duration::from_secs(6))
        .expect("the completed repeat exposes the true oldest again");
    assert_eq!(replacement.victim().map(|lease| lease.peer()), Some(oldest));
    replacement
        .begin()
        .expect("the exact oldest replacement begins")
        .finish();
    assert!(!registry.contains_at(oldest, start + Duration::from_secs(7)));
    assert!(registry.contains_at(newer, start + Duration::from_secs(7)));
    assert!(registry.contains_at(newcomer, start + Duration::from_secs(7)));
    assert!(registry.semantically_consistent());
}

#[test]
fn disjoint_free_slots_preserve_reservation_order_under_reverse_completion() {
    let registry = PeerBanSlotBank::with_limit_for_test(2);
    let start = Instant::now();
    let first = PeerIndex::from(20);
    let second = PeerIndex::from(21);
    let third = PeerIndex::from(22);
    let first_stage = registry
        .plan_record(first, start)
        .expect("the first free slot reserves");
    let second_stage = registry
        .plan_record(second, start + Duration::from_secs(1))
        .expect("the disjoint free slot reserves concurrently");
    second_stage
        .begin()
        .expect("the later reservation begins first")
        .finish();
    first_stage
        .begin()
        .expect("the earlier reservation begins second")
        .finish();
    let replacement = registry
        .plan_record(third, start + Duration::from_secs(2))
        .expect("the full bank selects by reservation order");
    assert_eq!(replacement.victim().map(|lease| lease.peer()), Some(first));
    drop(replacement);
    assert!(registry.semantically_consistent());
}

#[test]
fn disjoint_expired_slots_preserve_reservation_order_under_reverse_completion() {
    let registry = PeerBanSlotBank::with_limit_for_test(2);
    let start = Instant::now();
    for (offset, peer) in [(0, PeerIndex::from(23)), (1, PeerIndex::from(24))] {
        registry
            .plan_record(peer, start + Duration::from_secs(offset))
            .expect("the fixture fills one exact slot")
            .begin()
            .expect("the exact reservation begins")
            .finish();
    }
    let after_expiry = start + Duration::from_secs(MALFORMED_TX_BAN_SECONDS + 2);
    let first = PeerIndex::from(25);
    let second = PeerIndex::from(26);
    let first_stage = registry
        .plan_record(first, after_expiry)
        .expect("the oldest expired slot reserves");
    let second_stage = registry
        .plan_record(second, after_expiry + Duration::from_secs(1))
        .expect("the other expired slot reserves independently");
    second_stage
        .begin()
        .expect("the later expired-slot reservation begins first")
        .finish();
    first_stage
        .begin()
        .expect("the earlier expired-slot reservation begins second")
        .finish();
    let replacement = registry
        .plan_record(PeerIndex::from(27), after_expiry + Duration::from_secs(2))
        .expect("the full bank preserves reservation order");
    assert_eq!(replacement.victim().map(|lease| lease.peer()), Some(first));
    drop(replacement);
    assert!(registry.semantically_consistent());
}

#[test]
fn full_bank_replacement_rollback_restores_the_exact_oldest() {
    let registry = PeerBanSlotBank::with_limit_for_test(2);
    let start = Instant::now();
    let first = PeerIndex::from(30);
    let second = PeerIndex::from(31);
    for (offset, peer) in [(0, first), (1, second)] {
        registry
            .plan_record(peer, start + Duration::from_secs(offset))
            .expect("the fixture fills one exact slot")
            .begin()
            .expect("the exact reservation begins")
            .finish();
    }
    let rolled_back = registry
        .plan_record(PeerIndex::from(32), start + Duration::from_secs(2))
        .expect("the first replacement reserves the oldest");
    assert_eq!(rolled_back.victim().map(|lease| lease.peer()), Some(first));
    drop(rolled_back);
    let replacement = registry
        .plan_record(PeerIndex::from(33), start + Duration::from_secs(3))
        .expect("rollback restores the same oldest slot");
    assert_eq!(replacement.victim().map(|lease| lease.peer()), Some(first));
    drop(replacement);
    assert!(registry.semantically_consistent());
}

#[test]
fn abandoning_a_begun_slot_faults_the_bank_closed() {
    let registry = PeerBanSlotBank::with_limit_for_test(1);
    let permit = registry
        .plan_record(PeerIndex::from(40), Instant::now())
        .expect("one slot reserves")
        .begin()
        .expect("the reservation begins");
    drop(permit);
    assert!(matches!(
        registry.plan_record(PeerIndex::from(41), Instant::now()),
        Err(PeerBanError::Faulted)
    ));
    assert!(!registry.semantically_consistent());
}

#[test]
fn faulted_bank_rejects_a_preexisting_unbegun_reservation() {
    let registry = PeerBanSlotBank::with_limit_for_test(2);
    let abandoned = registry
        .plan_record(PeerIndex::from(42), Instant::now())
        .expect("the first slot reserves")
        .begin()
        .expect("the first reservation begins");
    let pending = registry
        .plan_record(PeerIndex::from(43), Instant::now())
        .expect("a disjoint slot reserves before the bank faults");

    drop(abandoned);
    assert!(matches!(pending.begin(), Err(PeerBanError::Faulted)));
    assert!(matches!(
        registry.plan_record(PeerIndex::from(44), Instant::now()),
        Err(PeerBanError::Faulted)
    ));
    assert!(!registry.semantically_consistent());
}
