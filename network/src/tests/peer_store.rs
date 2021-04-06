use super::random_addr;
use crate::{
    extract_peer_id,
    multiaddr::Multiaddr,
    peer_store::{PeerStore, Status, ADDR_COUNT_LIMIT},
    Behaviour, PeerId, SessionType,
};

#[test]
fn test_add_connected_peer() {
    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 0);
    peer_store
        .add_connected_peer(addr, SessionType::Outbound)
        .unwrap();
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 1);
}

#[test]
fn test_add_addr() {
    let mut peer_store: PeerStore = Default::default();
    assert_eq!(peer_store.fetch_addrs_to_attempt(2).len(), 0);
    let addr = random_addr();
    peer_store.add_addr(addr).unwrap();
    assert_eq!(peer_store.fetch_addrs_to_attempt(2).len(), 1);
    // we have not connected yet, so return 0
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 0);
}

#[test]
fn test_report() {
    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    assert!(peer_store.report(&addr, Behaviour::TestGood).is_ok());
}

#[test]
fn test_update_status() {
    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    peer_store
        .add_connected_peer(addr.clone(), SessionType::Inbound)
        .unwrap();
    assert_eq!(
        peer_store.peer_status(&extract_peer_id(&addr).unwrap()),
        Status::Connected
    );
}

#[test]
fn test_ban_peer() {
    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    peer_store
        .add_connected_peer(addr.clone(), SessionType::Inbound)
        .unwrap();
    peer_store
        .ban_addr(&addr, 10_000, "no reason".into())
        .unwrap();
    assert!(peer_store.is_addr_banned(&addr));
}

#[test]
fn test_attempt_ban() {
    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    peer_store.add_addr(addr.clone()).unwrap();
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
    let addr = random_addr();
    peer_store.add_addr(addr.clone()).unwrap();
    assert_eq!(peer_store.fetch_addrs_to_attempt(2).len(), 1);
    peer_store
        .add_connected_peer(addr, SessionType::Outbound)
        .unwrap();
    assert!(peer_store.fetch_addrs_to_attempt(1).is_empty());
}

#[test]
fn test_fetch_addrs_to_attempt_in_last_minutes() {
    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    peer_store.add_addr(addr.clone()).unwrap();
    let now = faketime::unix_time_as_millis();

    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&addr) {
        paddr.mark_tried(now);
    }
    assert!(peer_store.fetch_addrs_to_attempt(1).is_empty());
    // after 60 seconds
    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&addr) {
        paddr.mark_tried(now - 60_001);
    }
    assert_eq!(peer_store.fetch_addrs_to_attempt(1).len(), 1);
}

#[test]
fn test_fetch_addrs_to_feeler() {
    let mut peer_store: PeerStore = Default::default();
    assert!(peer_store.fetch_addrs_to_feeler(1).is_empty());
    let addr = random_addr();

    // add an addr
    peer_store.add_addr(addr.clone()).unwrap();
    assert_eq!(peer_store.fetch_addrs_to_feeler(2).len(), 1);

    // ignores connected peers' addrs
    peer_store
        .add_connected_peer(addr.clone(), SessionType::Outbound)
        .unwrap();
    assert!(peer_store.fetch_addrs_to_feeler(1).is_empty());

    // peer does not need feeler if it connected to us recently
    peer_store.remove_disconnected_peer(&addr);
    assert!(peer_store.fetch_addrs_to_feeler(1).is_empty());
}

#[test]
fn test_fetch_random_addrs() {
    let mut peer_store: PeerStore = Default::default();
    assert!(peer_store.fetch_random_addrs(1).is_empty());
    let addr1: Multiaddr = format!("/ip4/225.0.0.1/tcp/42/p2p/{}", PeerId::random().to_base58())
        .parse()
        .unwrap();
    let addr2: Multiaddr = format!("/ip4/225.0.0.2/tcp/42/p2p/{}", PeerId::random().to_base58())
        .parse()
        .unwrap();
    let addr3: Multiaddr = format!("/ip4/225.0.0.3/tcp/42/p2p/{}", PeerId::random().to_base58())
        .parse()
        .unwrap();
    // random should not return peer that we have never connected to
    assert!(peer_store.fetch_random_addrs(1).is_empty());
    // can't get peer addr from inbound
    peer_store
        .add_connected_peer(addr1.clone(), SessionType::Inbound)
        .unwrap();
    assert!(peer_store.fetch_random_addrs(1).is_empty());
    // get peer addr from outbound
    peer_store
        .add_connected_peer(addr1, SessionType::Outbound)
        .unwrap();
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 1);
    // get peer addrs by limit
    peer_store
        .add_connected_peer(addr2, SessionType::Outbound)
        .unwrap();
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 2);
    assert_eq!(peer_store.fetch_random_addrs(1).len(), 1);

    // return old peer's addr
    peer_store.add_addr(addr3.clone()).unwrap();
    peer_store
        .add_connected_peer(addr3.clone(), SessionType::Outbound)
        .unwrap();
    // set last_connected_at_ms to an expired timestamp
    // should still return peer's addr
    peer_store
        .mut_addr_manager()
        .get_mut(&addr3)
        .unwrap()
        .last_connected_at_ms = 0;
    assert_eq!(peer_store.fetch_random_addrs(3).len(), 3);
    peer_store.remove_disconnected_peer(&addr3);
    assert_eq!(peer_store.fetch_random_addrs(3).len(), 2);
}

#[test]
fn test_get_random_restrict_addrs_from_same_ip() {
    let mut peer_store: PeerStore = Default::default();
    let addr1 = format!("/ip4/225.0.0.1/tcp/42/p2p/{}", PeerId::random().to_base58())
        .parse()
        .unwrap();
    let addr2 = format!("/ip4/225.0.0.1/tcp/43/p2p/{}", PeerId::random().to_base58())
        .parse()
        .unwrap();
    peer_store
        .add_connected_peer(addr1, SessionType::Outbound)
        .unwrap();
    peer_store
        .add_connected_peer(addr2, SessionType::Outbound)
        .unwrap();
    assert_eq!(peer_store.fetch_random_addrs(2).len(), 1);
}

#[test]
fn test_eviction() {
    let mut peer_store = PeerStore::default();
    let now = faketime::unix_time_as_millis();
    let tried_ms = now - 61_000;
    // add addrs
    for i in 0..(ADDR_COUNT_LIMIT - 2) {
        let addr: Multiaddr = format!(
            "/ip4/225.0.0.1/tcp/{}/p2p/{}",
            i,
            PeerId::random().to_base58()
        )
        .parse()
        .unwrap();
        peer_store.add_addr(addr).unwrap();
    }
    // this peer will be evict from peer store
    let evict_addr: Multiaddr =
        format!("/ip4/225.0.0.2/tcp/42/p2p/{}", PeerId::random().to_base58())
            .parse()
            .unwrap();
    peer_store.add_addr(evict_addr.clone()).unwrap();
    // this peer will reserve in peer store
    let reserved_addr: Multiaddr = format!(
        "/ip4/192.163.1.1/tcp/42/p2p/{}",
        PeerId::random().to_base58()
    )
    .parse()
    .unwrap();
    peer_store.add_addr(reserved_addr.clone()).unwrap();
    // mark two peers as terrible peer
    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&evict_addr) {
        paddr.mark_tried(tried_ms);
        paddr.mark_tried(tried_ms);
        paddr.mark_tried(tried_ms);
    }
    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&reserved_addr) {
        paddr.mark_tried(tried_ms);
        paddr.mark_tried(tried_ms);
        paddr.mark_tried(tried_ms);
        assert!(paddr.is_terrible(now));
    }
    // should evict evict_addr and accept new_peer
    let new_peer_addr: Multiaddr =
        format!("/ip4/225.0.0.3/tcp/42/p2p/{}", PeerId::random().to_base58())
            .parse()
            .unwrap();
    peer_store.add_addr(new_peer_addr.clone()).unwrap();
    // check addrs
    // peer store will only evict peers from largest network group
    // the evict_addr should be evict, other two addrs will remain in peer store
    assert!(peer_store.mut_addr_manager().get(&new_peer_addr).is_some());
    assert!(peer_store.mut_addr_manager().get(&reserved_addr).is_some());
    assert!(peer_store.mut_addr_manager().get(&evict_addr).is_none());
}
