use crate::{
    multiaddr::{self, Multiaddr},
    peer_store::{types::MultiaddrExt, PeerStore, Status, ADDR_COUNT_LIMIT},
    Behaviour, PeerId, SessionType,
};

#[test]
fn test_add_connected_peer() {
    let mut peer_store: PeerStore = Default::default();
    let peer_id = PeerId::random();
    let addr = "/ip4/127.0.0.1/tcp/42".parse().unwrap();
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 0);
    peer_store
        .add_connected_peer(peer_id, addr, SessionType::Outbound)
        .unwrap();
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 1);
}

#[test]
fn test_add_addr() {
    let mut peer_store: PeerStore = Default::default();
    let peer_id = PeerId::random();
    assert_eq!(peer_store.fetch_addrs_to_attempt(2).len(), 0);
    peer_store
        .add_addr(peer_id, "/ip4/127.0.0.1/tcp/42".parse().unwrap())
        .unwrap();
    assert_eq!(peer_store.fetch_addrs_to_attempt(2).len(), 1);
    // we have not connected yet, so return 0
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 0);
}

#[test]
fn test_report() {
    let mut peer_store: PeerStore = Default::default();
    let peer_id = PeerId::random();
    assert!(peer_store.report(&peer_id, Behaviour::TestGood).is_ok());
}

#[test]
fn test_update_status() {
    let mut peer_store: PeerStore = Default::default();
    let peer_id = PeerId::random();
    // peer_store.update_status(&peer_id, Status::Connected);
    // assert_eq!(peer_store.peer_status(&peer_id), Status::Unknown);
    let addr = "/ip4/127.0.0.1/tcp/42".parse().unwrap();
    peer_store
        .add_connected_peer(peer_id.clone(), addr, SessionType::Inbound)
        .unwrap();
    assert_eq!(peer_store.peer_status(&peer_id), Status::Connected);
}

#[test]
fn test_ban_peer() {
    let mut peer_store: PeerStore = Default::default();
    let peer_id = PeerId::random();
    let addr: Multiaddr = "/ip4/127.0.0.1/tcp/42".parse().unwrap();
    peer_store
        .add_connected_peer(peer_id, addr.clone(), SessionType::Inbound)
        .unwrap();
    peer_store
        .ban_addr(&addr, 10_000, "no reason".into())
        .unwrap();
    assert!(peer_store.is_addr_banned(&addr));
}

#[test]
fn test_attempt_ban() {
    let mut peer_store: PeerStore = Default::default();
    let peer_id = PeerId::random();
    let addr = "/ip4/127.0.0.1/tcp/42".parse::<Multiaddr>().unwrap();
    peer_store.add_addr(peer_id, addr.clone()).unwrap();
    assert_eq!(peer_store.fetch_addrs_to_attempt(2).len(), 1);
    peer_store
        .ban_addr(&addr, 10_000, "no reason".into())
        .unwrap();
    assert_eq!(peer_store.fetch_addrs_to_attempt(2).len(), 0);
}

#[test]
fn test_fetch_addrs_to_attempt() {
    let mut peer_store: PeerStore = Default::default();
    assert!(peer_store.fetch_addrs_to_attempt(1).is_empty());
    let addr = "/ip4/127.0.0.1/tcp/42".parse::<Multiaddr>().unwrap();
    let peer_id = PeerId::random();
    peer_store.add_addr(peer_id.clone(), addr.clone()).unwrap();
    assert_eq!(peer_store.fetch_addrs_to_attempt(2).len(), 1);
    peer_store
        .add_connected_peer(peer_id, addr, SessionType::Outbound)
        .unwrap();
    assert!(peer_store.fetch_addrs_to_attempt(1).is_empty());
}

#[test]
fn test_fetch_addrs_to_attempt_in_last_minutes() {
    let mut peer_store: PeerStore = Default::default();
    let peer_id = PeerId::random();
    let addr = "/ip4/127.0.0.1/tcp/42".parse::<Multiaddr>().unwrap();
    peer_store.add_addr(peer_id, addr).unwrap();
    let paddr = peer_store.fetch_addrs_to_attempt(1).remove(0);
    let now = faketime::unix_time_as_millis();

    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&paddr.ip_port()) {
        paddr.mark_tried(now);
    }
    assert!(peer_store.fetch_addrs_to_attempt(1).is_empty());
    // after 60 seconds
    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&paddr.ip_port()) {
        paddr.mark_tried(now - 60_001);
    }
    assert_eq!(peer_store.fetch_addrs_to_attempt(1).len(), 1);
}

#[test]
fn test_fetch_addrs_to_feeler() {
    let mut peer_store: PeerStore = Default::default();
    assert!(peer_store.fetch_addrs_to_feeler(1).is_empty());
    let addr = "/ip4/127.0.0.1/tcp/42".parse::<Multiaddr>().unwrap();

    // add an addr
    let peer_id = PeerId::random();
    peer_store.add_addr(peer_id.clone(), addr.clone()).unwrap();
    assert_eq!(peer_store.fetch_addrs_to_feeler(2).len(), 1);

    // ignores connected peers' addrs
    peer_store
        .add_connected_peer(peer_id.clone(), addr, SessionType::Outbound)
        .unwrap();
    assert!(peer_store.fetch_addrs_to_feeler(1).is_empty());

    // peer does not need feeler if it connected to us recently
    peer_store.remove_disconnected_peer(&peer_id);
    assert!(peer_store.fetch_addrs_to_feeler(1).is_empty());
}

#[test]
fn test_fetch_random_addrs() {
    let mut peer_store: PeerStore = Default::default();
    assert!(peer_store.fetch_random_addrs(1).is_empty());
    let addr = "/ip4/225.0.0.1/tcp/42".parse::<Multiaddr>().unwrap();
    let peer_id = PeerId::random();
    let addr2 = "/ip4/225.0.0.2/tcp/42".parse::<Multiaddr>().unwrap();
    let peer_id2 = PeerId::random();
    let addr3 = "/ip4/225.0.0.3/tcp/42".parse::<Multiaddr>().unwrap();
    let peer_id3 = PeerId::random();
    let duplicated_addr = "/ip4/225.0.0.1/tcp/41".parse::<Multiaddr>().unwrap();
    // random should not return peer that we have never connected to
    assert!(peer_store.fetch_random_addrs(1).is_empty());
    // can't get peer addr from inbound
    peer_store
        .add_connected_peer(peer_id.clone(), addr.clone(), SessionType::Inbound)
        .unwrap();
    assert!(peer_store.fetch_random_addrs(1).is_empty());
    // get peer addr from outbound
    peer_store
        .add_connected_peer(peer_id, addr, SessionType::Outbound)
        .unwrap();
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 1);
    // filter duplicated addr
    peer_store
        .add_connected_peer(PeerId::random(), duplicated_addr, SessionType::Outbound)
        .unwrap();
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 1);
    // get peer addrs by limit
    peer_store
        .add_connected_peer(peer_id2, addr2, SessionType::Outbound)
        .unwrap();
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 2);
    assert_eq!(peer_store.fetch_random_addrs(1).len(), 1);

    // return old peer's addr
    peer_store
        .add_addr(peer_id3.clone(), addr3.clone())
        .unwrap();
    peer_store
        .add_connected_peer(peer_id3.clone(), addr3.clone(), SessionType::Outbound)
        .unwrap();
    // set last_connected_at_ms to an expired timestamp
    // should still return peer's addr
    peer_store
        .mut_addr_manager()
        .get_mut(&addr3.extract_ip_addr().unwrap())
        .unwrap()
        .last_connected_at_ms = 0;
    assert_eq!(peer_store.fetch_random_addrs(3).len(), 3);
    peer_store.remove_disconnected_peer(&peer_id3);
    assert_eq!(peer_store.fetch_random_addrs(3).len(), 2);
}

#[test]
fn test_get_random_restrict_addrs_from_same_ip() {
    let mut peer_store: PeerStore = Default::default();
    let peer_id = PeerId::random();
    let peer_id2 = PeerId::random();
    let addr = "/ip4/225.0.0.1/tcp/42".parse::<Multiaddr>().unwrap();
    let addr2 = "/ip4/225.0.0.1/tcp/43".parse::<Multiaddr>().unwrap();
    peer_store
        .add_connected_peer(peer_id, addr, SessionType::Outbound)
        .unwrap();
    peer_store
        .add_connected_peer(peer_id2, addr2, SessionType::Outbound)
        .unwrap();
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 1);
}

#[test]
fn test_trim_p2p_phase() {
    let mut peer_store: PeerStore = Default::default();
    let peer_id = PeerId::random();
    let addr = format!("/ip4/225.0.0.1/tcp/42/p2p/{}", peer_id.to_base58())
        .parse::<Multiaddr>()
        .unwrap();
    peer_store.add_addr(peer_id, addr).unwrap();
    let addr = peer_store.fetch_addrs_to_attempt(1).remove(0);
    let has_p2p_phase = addr.addr.into_iter().find(|proto| match proto {
        multiaddr::Protocol::P2P(_) => true,
        _ => false,
    });
    assert!(has_p2p_phase.is_none());
}

#[test]
fn test_eviction() {
    let mut peer_store = PeerStore::default();
    let now = faketime::unix_time_as_millis();
    let tried_ms = now - 61_000;
    // add addrs
    for i in 0..(ADDR_COUNT_LIMIT - 2) {
        let addr: Multiaddr = format!("/ip4/225.0.0.1/tcp/{}", i).parse().unwrap();
        peer_store.add_addr(PeerId::random(), addr).unwrap();
    }
    // this peer will be evict from peer store
    let evict_addr: Multiaddr = "/ip4/225.0.0.2/tcp/42".parse().unwrap();
    peer_store
        .add_addr(PeerId::random(), evict_addr.clone())
        .unwrap();
    // this peer will reserve in peer store
    let reserved_addr: Multiaddr = "/ip4/192.163.1.1/tcp/42".parse().unwrap();
    peer_store
        .add_addr(PeerId::random(), reserved_addr.clone())
        .unwrap();
    // mark two peers as terrible peer
    if let Some(paddr) = peer_store
        .mut_addr_manager()
        .get_mut(&evict_addr.extract_ip_addr().unwrap())
    {
        paddr.mark_tried(tried_ms);
        paddr.mark_tried(tried_ms);
        paddr.mark_tried(tried_ms);
    }
    if let Some(paddr) = peer_store
        .mut_addr_manager()
        .get_mut(&reserved_addr.extract_ip_addr().unwrap())
    {
        paddr.mark_tried(tried_ms);
        paddr.mark_tried(tried_ms);
        paddr.mark_tried(tried_ms);
        assert!(paddr.is_terrible(now));
    }
    // should evict evict_addr and accept new_peer
    let new_peer_addr: Multiaddr = "/ip4/225.0.0.3/tcp/42".parse().unwrap();
    peer_store
        .add_addr(PeerId::random(), new_peer_addr.clone())
        .unwrap();
    // check addrs
    // peer store will only evict peers from largest network group
    // the evict_addr should be evict, other two addrs will remain in peer store
    assert!(peer_store
        .mut_addr_manager()
        .get(&new_peer_addr.extract_ip_addr().unwrap())
        .is_some());
    assert!(peer_store
        .mut_addr_manager()
        .get(&reserved_addr.extract_ip_addr().unwrap())
        .is_some());
    assert!(peer_store
        .mut_addr_manager()
        .get(&evict_addr.extract_ip_addr().unwrap())
        .is_none());
}
