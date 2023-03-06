use super::{random_addr, random_addr_v6};
use crate::{
    extract_peer_id,
    multiaddr::Multiaddr,
    peer_store::{
        ban_list::CLEAR_INTERVAL_COUNTER, types::multiaddr_to_ip_network, PeerStore, Status,
        ADDR_COUNT_LIMIT, ADDR_TRY_TIMEOUT_MS,
    },
    Behaviour, Flags, PeerId, SessionType,
};
use std::collections::HashSet;

#[test]
fn test_add_connected_peer() {
    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    assert_eq!(
        peer_store.fetch_random_addrs(2, Flags::COMPATIBILITY).len(),
        0
    );
    peer_store.add_connected_peer(addr.clone(), SessionType::Outbound);
    peer_store.add_addr(addr, Flags::COMPATIBILITY).unwrap();
    assert_eq!(
        peer_store.fetch_random_addrs(2, Flags::COMPATIBILITY).len(),
        1
    );
}

#[test]
fn test_add_addr() {
    let mut peer_store: PeerStore = Default::default();
    assert_eq!(
        peer_store
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY)
            .len(),
        0
    );
    let addr = random_addr();
    peer_store.add_addr(addr, Flags::COMPATIBILITY).unwrap();
    assert_eq!(peer_store.fetch_addrs_to_feeler(2).len(), 1);
    // we have not connected yet, so return 0
    assert_eq!(
        peer_store
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY)
            .len(),
        0
    );
    assert_eq!(
        peer_store.fetch_random_addrs(2, Flags::COMPATIBILITY).len(),
        0
    );
}

#[test]
fn test_report() {
    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr_v6();
    peer_store
        .add_addr(addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    assert!(peer_store.report(&addr, Behaviour::TestGood).is_ok());

    for _ in 0..7 {
        assert!(peer_store.report(&addr, Behaviour::TestBad).is_ok());
    }

    assert!(peer_store.report(&addr, Behaviour::TestBad).is_banned());
    assert!(peer_store
        .add_addr(addr.clone(), Flags::COMPATIBILITY)
        .is_ok());
    assert!(peer_store.addr_manager().get(&addr).is_none())
}

#[test]
fn test_update_status() {
    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    peer_store.add_connected_peer(addr.clone(), SessionType::Inbound);
    assert_eq!(
        peer_store.peer_status(&extract_peer_id(&addr).unwrap()),
        Status::Connected
    );
}

#[cfg(not(disable_faketime))]
#[test]
fn test_ban_peer() {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(0);

    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    peer_store.add_connected_peer(addr.clone(), SessionType::Inbound);
    peer_store.ban_addr(&addr, 10_000, "no reason".into());
    assert!(peer_store.is_addr_banned(&addr));
    peer_store
        .mut_ban_list()
        .unban_network(&multiaddr_to_ip_network(&addr).unwrap());
    assert!(!peer_store.is_addr_banned(&addr));

    let mut set = HashSet::with_capacity(CLEAR_INTERVAL_COUNTER);
    for _ in 0..CLEAR_INTERVAL_COUNTER - 2 {
        let addr: Multiaddr = loop {
            let addr = std::net::Ipv4Addr::new(
                rand::random(),
                rand::random(),
                rand::random(),
                rand::random(),
            );
            if set.insert(addr) {
                break Multiaddr::from(addr);
            }
        };
        peer_store.ban_addr(&addr, 10_000, "no reason".into());
    }

    _faketime_guard.set_faketime(30_000);

    // Cleanup will be performed every 1024 inserts
    let addr = random_addr_v6();
    peer_store.ban_addr(&addr, 10_000, "no reason".into());
    assert_eq!(peer_store.ban_list().count(), 1)
}

#[cfg(not(disable_faketime))]
#[test]
fn test_attempt_ban() {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(1);

    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    peer_store
        .add_addr(addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    peer_store
        .mut_addr_manager()
        .get_mut(&addr)
        .unwrap()
        .mark_connected(ckb_systemtime::unix_time_as_millis());

    _faketime_guard.set_faketime(100_000);

    assert_eq!(
        peer_store
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY)
            .len(),
        1
    );
    peer_store.ban_addr(&addr, 10_000, "no reason".into());
    assert_eq!(
        peer_store
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY)
            .len(),
        0
    );
}

#[cfg(not(disable_faketime))]
#[test]
fn test_fetch_addrs_to_attempt() {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(1);

    let mut peer_store: PeerStore = Default::default();
    assert!(peer_store
        .fetch_addrs_to_attempt(1, Flags::COMPATIBILITY)
        .is_empty());
    let addr = random_addr();
    peer_store
        .add_addr(addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    peer_store
        .mut_addr_manager()
        .get_mut(&addr)
        .unwrap()
        .mark_connected(ckb_systemtime::unix_time_as_millis());
    _faketime_guard.set_faketime(100_000);

    assert_eq!(
        peer_store
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY)
            .len(),
        1
    );
    peer_store.add_connected_peer(addr, SessionType::Outbound);
    assert!(peer_store
        .fetch_addrs_to_attempt(1, Flags::COMPATIBILITY)
        .is_empty());
}

#[cfg(not(disable_faketime))]
#[test]
fn test_fetch_addrs_to_attempt_or_feeler() {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(1);

    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    peer_store.add_outbound_addr(addr, Flags::COMPATIBILITY);

    _faketime_guard.set_faketime(100_000);

    assert_eq!(
        peer_store
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY)
            .len(),
        1
    );
    assert!(peer_store.fetch_addrs_to_feeler(2).is_empty());

    _faketime_guard.set_faketime(100_000 + ADDR_TRY_TIMEOUT_MS + 1);

    assert!(peer_store
        .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY)
        .is_empty());
    assert_eq!(peer_store.fetch_addrs_to_feeler(2).len(), 1);
}

#[cfg(not(disable_faketime))]
#[test]
fn test_fetch_addrs_to_attempt_in_last_minutes() {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(100_000);

    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    peer_store
        .add_addr(addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    let now = ckb_systemtime::unix_time_as_millis();

    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&addr) {
        paddr.mark_tried(now);
    }
    assert!(peer_store
        .fetch_addrs_to_attempt(1, Flags::COMPATIBILITY)
        .is_empty());
    // after 60 seconds
    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&addr) {
        paddr.mark_tried(now - 60_001);
    }
    assert!(peer_store
        .fetch_addrs_to_attempt(1, Flags::COMPATIBILITY)
        .is_empty());
    peer_store
        .mut_addr_manager()
        .get_mut(&addr)
        .unwrap()
        .mark_connected(now);
    _faketime_guard.set_faketime(200_000);

    assert_eq!(
        peer_store
            .fetch_addrs_to_attempt(1, Flags::COMPATIBILITY)
            .len(),
        1
    );
    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&addr) {
        paddr.mark_tried(now);
    }
    assert_eq!(
        peer_store
            .fetch_addrs_to_attempt(1, Flags::COMPATIBILITY)
            .len(),
        1
    );
}

#[test]
fn test_fetch_addrs_to_feeler() {
    let mut peer_store: PeerStore = Default::default();
    assert!(peer_store.fetch_addrs_to_feeler(1).is_empty());
    let addr = random_addr();

    // add an addr
    peer_store
        .add_addr(addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    assert_eq!(peer_store.fetch_addrs_to_feeler(2).len(), 1);

    // ignores connected peers' addrs
    peer_store.add_connected_peer(addr.clone(), SessionType::Outbound);
    assert!(peer_store.fetch_addrs_to_feeler(1).is_empty());

    // peer does not need feeler if it connected to us recently
    peer_store
        .mut_addr_manager()
        .get_mut(&addr)
        .unwrap()
        .last_connected_at_ms = ckb_systemtime::unix_time_as_millis();
    peer_store.remove_disconnected_peer(&addr);
    assert!(peer_store.fetch_addrs_to_feeler(1).is_empty());
}

#[test]
fn test_fetch_random_addrs() {
    let mut peer_store: PeerStore = Default::default();
    assert!(peer_store
        .fetch_random_addrs(1, Flags::COMPATIBILITY)
        .is_empty());
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
    assert!(peer_store
        .fetch_random_addrs(1, Flags::COMPATIBILITY)
        .is_empty());
    // can't get peer addr from inbound
    peer_store.add_connected_peer(addr1.clone(), SessionType::Inbound);
    assert!(peer_store
        .fetch_random_addrs(1, Flags::COMPATIBILITY)
        .is_empty());
    // get peer addr from outbound
    peer_store.add_connected_peer(addr1.clone(), SessionType::Outbound);
    peer_store.add_addr(addr1, Flags::COMPATIBILITY).unwrap();
    assert_eq!(
        peer_store.fetch_random_addrs(2, Flags::COMPATIBILITY).len(),
        1
    );
    // get peer addrs by limit
    peer_store.add_connected_peer(addr2.clone(), SessionType::Outbound);
    peer_store.add_addr(addr2, Flags::COMPATIBILITY).unwrap();
    assert_eq!(
        peer_store.fetch_random_addrs(2, Flags::COMPATIBILITY).len(),
        2
    );
    assert_eq!(
        peer_store.fetch_random_addrs(1, Flags::COMPATIBILITY).len(),
        1
    );

    // return old peer's addr
    peer_store
        .add_addr(addr3.clone(), Flags::COMPATIBILITY)
        .unwrap();
    peer_store.add_connected_peer(addr3.clone(), SessionType::Outbound);
    // set last_connected_at_ms to an expired timestamp
    // should still return peer's addr
    peer_store
        .mut_addr_manager()
        .get_mut(&addr3)
        .unwrap()
        .mark_connected(0);
    assert_eq!(
        peer_store.fetch_random_addrs(3, Flags::COMPATIBILITY).len(),
        3
    );
    peer_store.remove_disconnected_peer(&addr3);
    assert_eq!(
        peer_store.fetch_random_addrs(3, Flags::COMPATIBILITY).len(),
        2
    );
}

#[test]
fn test_random_fetch_with_filter() {
    let mut peer_store: PeerStore = Default::default();
    assert!(peer_store
        .fetch_random_addrs(1, Flags::COMPATIBILITY)
        .is_empty());
    let addr1: Multiaddr = format!("/ip4/225.0.0.1/tcp/42/p2p/{}", PeerId::random().to_base58())
        .parse()
        .unwrap();
    let addr2: Multiaddr = format!("/ip4/225.0.0.2/tcp/42/p2p/{}", PeerId::random().to_base58())
        .parse()
        .unwrap();
    let addr3: Multiaddr = format!("/ip4/225.0.0.3/tcp/42/p2p/{}", PeerId::random().to_base58())
        .parse()
        .unwrap();

    peer_store
        .add_addr(addr1.clone(), Flags::COMPATIBILITY)
        .unwrap();
    peer_store
        .mut_addr_manager()
        .get_mut(&addr1)
        .unwrap()
        .last_connected_at_ms = ckb_systemtime::unix_time_as_millis();
    assert_eq!(peer_store.addr_manager().count(), 1);
    assert_eq!(
        peer_store.fetch_random_addrs(1, Flags::COMPATIBILITY).len(),
        1
    );
    assert_eq!(peer_store.fetch_random_addrs(2, Flags::SYNC).len(), 0);

    peer_store
        .add_addr(addr2.clone(), Flags::COMPATIBILITY | Flags::SYNC)
        .unwrap();
    peer_store
        .mut_addr_manager()
        .get_mut(&addr2)
        .unwrap()
        .last_connected_at_ms = ckb_systemtime::unix_time_as_millis();
    assert_eq!(peer_store.fetch_random_addrs(2, Flags::SYNC).len(), 1);

    peer_store
        .add_addr(addr3.clone(), Flags::RELAY | Flags::SYNC)
        .unwrap();
    peer_store
        .mut_addr_manager()
        .get_mut(&addr3)
        .unwrap()
        .last_connected_at_ms = ckb_systemtime::unix_time_as_millis();
    assert_eq!(peer_store.fetch_random_addrs(2, Flags::SYNC).len(), 2);

    assert_eq!(
        peer_store
            .fetch_random_addrs(4, Flags::SYNC | Flags::COMPATIBILITY)
            .len(),
        1
    );
}

#[test]
fn test_get_random_restrict_addrs_from_same_ip() {
    let mut peer_store: PeerStore = Default::default();
    let addr1: Multiaddr = format!("/ip4/225.0.0.1/tcp/42/p2p/{}", PeerId::random().to_base58())
        .parse()
        .unwrap();
    let addr2: Multiaddr = format!("/ip4/225.0.0.1/tcp/43/p2p/{}", PeerId::random().to_base58())
        .parse()
        .unwrap();
    peer_store.add_connected_peer(addr1.clone(), SessionType::Outbound);
    peer_store.add_connected_peer(addr2.clone(), SessionType::Outbound);
    peer_store.add_addr(addr1, Flags::COMPATIBILITY).unwrap();
    peer_store.add_addr(addr2, Flags::COMPATIBILITY).unwrap();
    assert_eq!(
        peer_store.fetch_random_addrs(2, Flags::COMPATIBILITY).len(),
        1
    );
}

#[test]
fn test_eviction() {
    let mut peer_store = PeerStore::default();
    let now = ckb_systemtime::unix_time_as_millis();
    let tried_ms = now - 61_000;
    // add addrs, make the peer store has 4 groups addrs
    for i in 0..(ADDR_COUNT_LIMIT - 5) {
        let addr: Multiaddr = format!(
            "/ip4/225.0.0.1/tcp/{}/p2p/{}",
            i,
            PeerId::random().to_base58()
        )
        .parse()
        .unwrap();
        peer_store.add_addr(addr, Flags::COMPATIBILITY).unwrap();
    }
    let addr: Multiaddr = format!(
        "/ip4/192.163.1.1/tcp/43/p2p/{}",
        PeerId::random().to_base58()
    )
    .parse()
    .unwrap();
    peer_store.add_addr(addr, Flags::COMPATIBILITY).unwrap();
    let addr: Multiaddr = format!(
        "/ip4/255.255.0.1/tcp/43/p2p/{}",
        PeerId::random().to_base58()
    )
    .parse()
    .unwrap();
    peer_store.add_addr(addr, Flags::COMPATIBILITY).unwrap();
    let addr: Multiaddr = random_addr_v6();
    peer_store.add_addr(addr, Flags::COMPATIBILITY).unwrap();

    // this peer will be evict from peer store
    let evict_addr: Multiaddr =
        format!("/ip4/225.0.0.2/tcp/42/p2p/{}", PeerId::random().to_base58())
            .parse()
            .unwrap();
    peer_store
        .add_addr(evict_addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    // this peer will be evict from peer store
    let evict_addr_2: Multiaddr = format!(
        "/ip4/192.163.1.1/tcp/42/p2p/{}",
        PeerId::random().to_base58()
    )
    .parse()
    .unwrap();
    peer_store
        .add_addr(evict_addr_2.clone(), Flags::COMPATIBILITY)
        .unwrap();
    // mark two peers as terrible peer
    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&evict_addr) {
        paddr.mark_tried(tried_ms);
        paddr.mark_tried(tried_ms);
        paddr.mark_tried(tried_ms);
        assert!(!paddr.is_connectable(now));
    }
    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&evict_addr_2) {
        paddr.mark_tried(tried_ms);
        paddr.mark_tried(tried_ms);
        paddr.mark_tried(tried_ms);
        assert!(!paddr.is_connectable(now));
    }
    // should evict evict_addr and accept new_peer
    let new_peer_addr: Multiaddr =
        format!("/ip4/225.0.0.3/tcp/42/p2p/{}", PeerId::random().to_base58())
            .parse()
            .unwrap();
    peer_store
        .add_addr(new_peer_addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    // check addrs
    // peer store will evict all peers which are invalid
    assert!(peer_store.mut_addr_manager().get(&new_peer_addr).is_some());
    assert!(peer_store.mut_addr_manager().get(&evict_addr_2).is_none());
    assert!(peer_store.mut_addr_manager().get(&evict_addr).is_none());

    // In the absence of invalid nodes, too many nodes on the same network segment will be automatically evicted
    let new_peer_addr: Multiaddr =
        format!("/ip4/225.0.0.3/tcp/63/p2p/{}", PeerId::random().to_base58())
            .parse()
            .unwrap();
    peer_store
        .add_addr(new_peer_addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    assert!(peer_store.mut_addr_manager().get(&new_peer_addr).is_some());
    let new_peer_addr: Multiaddr =
        format!("/ip4/225.0.0.3/tcp/59/p2p/{}", PeerId::random().to_base58())
            .parse()
            .unwrap();
    peer_store
        .add_addr(new_peer_addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    assert!(peer_store.mut_addr_manager().get(&new_peer_addr).is_some());
}
