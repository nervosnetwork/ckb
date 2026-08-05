use super::super::ban::PeerBanRegistry;
use crate::constants::MALFORMED_TX_BAN_SECONDS;
use ckb_network::PeerIndex;
use std::time::{Duration, Instant};

#[test]
fn live_marker_is_never_shortened_and_expired_markers_are_pruned_on_record() {
    let mut registry = PeerBanRegistry::default();
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
    registry.apply(long);
    let short = registry
        .plan_record(first, start + Duration::from_secs(1))
        .expect("replacement needs no allocation");
    registry.apply(short);
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
    registry.apply(next);
    let after_expiry = start + Duration::from_secs(MALFORMED_TX_BAN_SECONDS + 1);
    assert!(!registry.contains_at(first, after_expiry));
    assert!(registry.contains_at(second, after_expiry));
    assert_eq!(registry.snapshot().len(), 1);
    assert!(registry.semantically_consistent());
}

#[test]
fn saturation_evicts_the_oldest_fence_and_keeps_a_hard_bound() {
    let mut registry = PeerBanRegistry::with_limit_for_test(2);
    let start = Instant::now();
    let first = PeerIndex::from(1);
    let second = PeerIndex::from(2);
    let third = PeerIndex::from(3);

    for (offset, peer) in [(0, first), (1, second), (2, third)] {
        let delta = registry
            .plan_record(peer, start + Duration::from_secs(offset))
            .expect("the fixed-capacity fixture reuses an owned slot");
        registry.apply(delta);
        assert!(registry.semantically_consistent());
    }

    assert!(!registry.contains_at(first, start + Duration::from_secs(3)));
    assert!(registry.contains_at(second, start + Duration::from_secs(3)));
    assert!(registry.contains_at(third, start + Duration::from_secs(3)));
    assert_eq!(registry.snapshot().len(), 2);
}
