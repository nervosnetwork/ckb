use crate::{
    NetworkState, PeerId,
    protocols::hole_punching::{HolePunching, MAX_FORWARD_RATE_LIMITER_KEYS},
};
use ckb_app_config::NetworkConfig;
use std::sync::Arc;

fn hole_punching() -> HolePunching {
    let temp_dir = tempfile::Builder::new().tempdir().unwrap();
    let config = NetworkConfig {
        path: temp_dir.path().to_path_buf(),
        ..Default::default()
    };
    let network_state = Arc::new(NetworkState::from_config(config).unwrap());
    HolePunching::new(network_state)
}

#[test]
fn forward_rate_limiter_keeps_from_to_message_semantics() {
    let mut protocol = hole_punching();
    let from = PeerId::random();
    let to = PeerId::random();
    let now = 1_000;

    assert!(protocol.check_forward_rate_limit_at_for_test(&from, &to, 2, now));
    assert!(!protocol.check_forward_rate_limit_at_for_test(&from, &to, 2, now));

    assert!(protocol.check_forward_rate_limit_at_for_test(&PeerId::random(), &to, 2, now));
    assert!(protocol.check_forward_rate_limit_at_for_test(&from, &PeerId::random(), 2, now));
    assert!(protocol.check_forward_rate_limit_at_for_test(&from, &to, 3, now));
}

#[test]
fn forward_rate_limiter_bounds_message_controlled_peer_ids() {
    let mut protocol = hole_punching();
    let now = 1_000;

    assert_eq!(protocol.forward_rate_limiter_len(), 0);

    let message_controlled_peer_pairs =
        (0..MAX_FORWARD_RATE_LIMITER_KEYS).map(|_| (PeerId::random(), PeerId::random()));
    for (_from, _to) in message_controlled_peer_pairs {
        assert!(protocol.check_forward_rate_limit_at_for_test(&_from, &_to, 2, now));
    }

    assert_eq!(
        protocol.forward_rate_limiter_len(),
        MAX_FORWARD_RATE_LIMITER_KEYS
    );
    assert!(!protocol.check_forward_rate_limit_at_for_test(
        &PeerId::random(),
        &PeerId::random(),
        2,
        now
    ));
    assert_eq!(
        protocol.forward_rate_limiter_len(),
        MAX_FORWARD_RATE_LIMITER_KEYS
    );
}
