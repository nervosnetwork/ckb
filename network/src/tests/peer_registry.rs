use super::random_addr;
use crate::{
    errors::{Error, PeerError},
    extract_peer_id,
    multiaddr::Multiaddr,
    peer_registry::{PeerRegistry, EVICTION_PROTECT_PEERS},
    peer_store::PeerStore,
    PeerId, SessionType,
};
use std::time::{Duration, Instant};

#[test]
fn test_accept_inbound_peer_in_reserve_only_mode() {
    let mut peer_store = PeerStore::default();
    let whitelist_addr = format!("/ip4/127.0.0.1/tcp/43/p2p/{}", PeerId::random().to_base58())
        .parse::<Multiaddr>()
        .unwrap();
    let session_id = 1.into();

    // whitelist_only mode: only accept whitelist_peer
    let mut peers = PeerRegistry::new(3, 3, true, vec![whitelist_addr.clone()]);
    let err = peers
        .accept_peer(
            random_addr(),
            session_id,
            SessionType::Inbound,
            &mut peer_store,
        )
        .unwrap_err();
    assert_eq!(
        format!("{}", err),
        format!("{}", Error::Peer(PeerError::NonReserved))
    );

    peers
        .accept_peer(
            whitelist_addr,
            session_id,
            SessionType::Inbound,
            &mut peer_store,
        )
        .expect("accept");
}

#[test]
fn test_accept_inbound_peer_until_full() {
    let mut peer_store = PeerStore::default();
    let whitelist_addr = format!("/ip4/127.0.0.1/tcp/43/p2p/{}", PeerId::random().to_base58())
        .parse::<Multiaddr>()
        .unwrap();
    // accept node until inbound connections is full
    let mut peers = PeerRegistry::new(3, 3, false, vec![whitelist_addr.clone()]);
    for session_id in 1..=3 {
        peers
            .accept_peer(
                random_addr(),
                session_id.into(),
                SessionType::Inbound,
                &mut peer_store,
            )
            .expect("accept");
    }

    let err = peers
        .accept_peer(
            random_addr(),
            3.into(),
            SessionType::Outbound,
            &mut peer_store,
        )
        .unwrap_err();
    assert_eq!(
        format!("{}", err),
        format!("{}", Error::Peer(PeerError::SessionExists(3.into()))),
    );

    // test evict a peer
    assert!(peers
        .accept_peer(
            random_addr(),
            4.into(),
            SessionType::Inbound,
            &mut peer_store,
        )
        .expect("Accept peer should ok")
        .is_some());
    // should still accept whitelist peer
    peers
        .accept_peer(
            whitelist_addr.clone(),
            5.into(),
            SessionType::Inbound,
            &mut peer_store,
        )
        .expect("accept");
    let err = peers
        .accept_peer(
            whitelist_addr.clone(),
            6.into(),
            SessionType::Inbound,
            &mut peer_store,
        )
        .unwrap_err();
    assert_eq!(
        format!("{}", err),
        format!(
            "{}",
            Error::Peer(PeerError::PeerIdExists(
                extract_peer_id(&whitelist_addr).unwrap()
            ))
        ),
    );
}

#[test]
fn test_accept_inbound_peer_eviction() {
    // eviction inbound peer
    // We build an unprotected evict targets set
    // PeerRegistry should
    // 1. evict from largest network groups
    // 2. never evict whitelist peer
    let mut peer_store = PeerStore::default();
    let whitelist_addr = format!("/ip4/127.0.0.1/tcp/43/p2p/{}", PeerId::random().to_base58())
        .parse::<Multiaddr>()
        .unwrap();
    let mut evict_targets = vec![random_addr()];
    // prepare protected peers
    let longest_connection_time_peers_count = 5;
    let protected_peers_count = 2 * (EVICTION_PROTECT_PEERS + longest_connection_time_peers_count);
    let mut peers_registry = PeerRegistry::new(
        (protected_peers_count) as u32,
        3,
        false,
        vec![whitelist_addr],
    );
    // prepare all peers
    for session_id in 0..protected_peers_count {
        assert!(peers_registry
            .accept_peer(
                random_addr(),
                session_id.into(),
                SessionType::Inbound,
                &mut peer_store,
            )
            .is_ok());
    }
    let peers: Vec<_> = {
        peers_registry
            .peers()
            .values()
            .map(|peer| peer.connected_addr.clone())
            .collect()
    };

    let mut peers_iter = peers.iter();
    // lowest ping peers
    for _ in 0..EVICTION_PROTECT_PEERS {
        let peer_addr = peers_iter.next().unwrap();
        let peer_id = extract_peer_id(peer_addr).unwrap();
        let session_id = peers_registry
            .get_key_by_peer_id(&peer_id)
            .expect("get_key_by_peer_id failed");
        if let Some(peer) = peers_registry.get_peer_mut(session_id) {
            peer.ping_rtt = Some(Duration::from_secs(0));
        };
    }

    // to prevent time error, we set now to 10ago.
    let now = Instant::now() - Duration::from_secs(10);
    // peers which most recently sent messages
    for _ in 0..EVICTION_PROTECT_PEERS {
        let peer_addr = peers_iter.next().unwrap();
        let peer_id = extract_peer_id(peer_addr).unwrap();
        let session_id = peers_registry
            .get_key_by_peer_id(&peer_id)
            .expect("get_key_by_peer_id failed");
        if let Some(peer) = peers_registry.get_peer_mut(session_id) {
            peer.last_ping_protocol_message_received_at = Some(now + Duration::from_secs(10));
        };
    }
    // protect half peers which have the longest connection time
    for _ in 0..longest_connection_time_peers_count {
        let peer_addr = peers_iter.next().unwrap();
        let peer_id = extract_peer_id(peer_addr).unwrap();
        let session_id = peers_registry
            .get_key_by_peer_id(&peer_id)
            .expect("get_key_by_peer_id failed");
        if let Some(peer) = peers_registry.get_peer_mut(session_id) {
            peer.connected_time = now - Duration::from_secs(10);
        };
    }
    // thoses peers will not be protect, we add them to evict_targets
    for _ in 0..longest_connection_time_peers_count {
        let peer_addr = peers_iter.next().unwrap();
        let peer_id = extract_peer_id(peer_addr).unwrap();
        evict_targets.push(peer_addr.to_owned());
        let session_id = peers_registry
            .get_key_by_peer_id(&peer_id)
            .expect("get_key_by_peer_id failed");
        if let Some(peer) = peers_registry.get_peer_mut(session_id) {
            peer.connected_time = now - Duration::from_secs(10);
        };
    }

    peers_registry
        .accept_peer(
            random_addr(),
            2000.into(),
            SessionType::Inbound,
            &mut peer_store,
        )
        .expect("accept");
    let len_after_eviction = evict_targets
        .iter()
        .filter_map(|peer_addr| {
            peers_registry.get_key_by_peer_id(&extract_peer_id(peer_addr).unwrap())
        })
        .count();
    // should evict from one of evict_targets
    assert_eq!(len_after_eviction, evict_targets.len() - 1);
}
