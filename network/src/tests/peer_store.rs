use super::{random_addr, random_addr_v6};
use crate::{
    Behaviour, Flags, PeerId, SessionType, extract_peer_id,
    multiaddr::Multiaddr,
    peer_store::{
        ADDR_COUNT_LIMIT, ADDR_TRY_TIMEOUT_MS, PeerStore, Status, ban_list::CLEAR_INTERVAL_COUNTER,
        types::multiaddr_to_ip_network,
    },
};
use std::collections::HashSet;

#[test]
fn test_proxy_identity_ban_expiry_and_address_isolation() {
    for ip in ["127.0.0.1", "::1", "10.0.0.2"] {
        let ip: std::net::IpAddr = ip.parse().unwrap();
        let protocol = if ip.is_ipv4() { "ip4" } else { "ip6" };
        let peer = PeerId::random();
        let addr: Multiaddr = format!("/{protocol}/{ip}/tcp/1000/p2p/{peer}")
            .parse()
            .unwrap();
        let other: Multiaddr = format!("/{protocol}/{ip}/tcp/1001/p2p/{}", PeerId::random())
            .parse()
            .unwrap();
        let moved: Multiaddr = format!("/ip4/192.0.2.1/tcp/2000/p2p/{peer}")
            .parse()
            .unwrap();
        let mut store = PeerStore::default().with_shared_proxy_addrs(&[ip]);
        store.ban_addr(&addr, u64::MAX, "test".into());
        assert!(store.is_addr_banned(&addr));
        assert!(store.is_addr_banned(&moved));
        assert!(!store.is_addr_banned(&other));
        assert!(!store.ban_list().is_ip_banned(&ip));
        assert!(store.ban_list().get_banned_addrs().is_empty());

        store.ban_addr(&addr, 0, "expired".into());
        assert!(!store.is_addr_banned(&addr));
        store.ban_addr(&addr, u64::MAX, "test".into());
        assert!(store.is_addr_banned(&addr));
        store.clear_ban_list();
        assert!(!store.is_addr_banned(&addr));
    }
}

#[test]
fn test_proxy_behaviour_ban_and_explicit_ip_ban() {
    let addr = random_addr();
    let other = random_addr();
    let mut store = PeerStore::default().with_shared_proxy_addrs(&["127.0.0.1".parse().unwrap()]);
    store.add_addr(addr.clone(), Flags::COMPATIBILITY).unwrap();
    for _ in 0..6 {
        assert!(store.report(&addr, Behaviour::TestBad).is_ok());
    }
    assert!(store.report(&addr, Behaviour::TestBad).is_banned());
    assert!(store.is_addr_banned(&addr));
    assert!(!store.is_addr_banned(&other));
    assert!(store.addr_manager().get(&addr).is_none());

    // Operator bans must still apply even when the IP belongs to a trusted proxy.
    store.ban_network(
        multiaddr_to_ip_network(&addr).unwrap(),
        10_000,
        "manual".into(),
    );
    assert!(store.is_addr_banned(&other));
}

#[test]
fn test_untrusted_address_still_banned_by_ip() {
    let mut store = PeerStore::default().with_shared_proxy_addrs(&["::1".parse().unwrap()]);
    store.ban_addr(&random_addr(), 10_000, "test".into());
    assert!(store.is_addr_banned(&random_addr()));
    assert_eq!(store.ban_list().count(), 1);
}

// An outbound `/onion3` peer is dialled through SOCKS5, so its connected address carries no
// IP at all. Such a peer used to escape banning entirely, whatever it did.
#[test]
fn test_outbound_onion_addr_is_banned_by_identity() {
    const ONION: &str = "vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd";
    let peer = PeerId::random();
    let addr: Multiaddr = format!("/onion3/{ONION}:1234/p2p/{peer}").parse().unwrap();
    let other: Multiaddr = format!("/onion3/{ONION}:1234/p2p/{}", PeerId::random())
        .parse()
        .unwrap();
    // The same identity reached over clearnet must be banned too.
    let moved: Multiaddr = format!("/ip4/192.0.2.1/tcp/2000/p2p/{peer}")
        .parse()
        .unwrap();

    // No onion listener configured: the fallback must not depend on `shared_proxy_addrs`.
    let mut store = PeerStore::default();
    assert!(!store.is_addr_banned(&addr));
    store.ban_addr(&addr, u64::MAX, "test".into());
    assert!(store.is_addr_banned(&addr));
    assert!(store.is_addr_banned(&moved));
    assert!(!store.is_addr_banned(&other));
    // Nothing IP based was recorded, there is no IP to record.
    assert!(store.ban_list().get_banned_addrs().is_empty());
    assert!(!store.is_addr_banned(&random_addr()));
}

// Behaviour reports on an onion peer must reach the identity ban as well.
#[test]
fn test_outbound_onion_behaviour_ban() {
    const ONION: &str = "vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd";
    let addr: Multiaddr = format!("/onion3/{ONION}:1234/p2p/{}", PeerId::random())
        .parse()
        .unwrap();
    let mut store = PeerStore::default();
    store.add_addr(addr.clone(), Flags::COMPATIBILITY).unwrap();
    for _ in 0..6 {
        assert!(store.report(&addr, Behaviour::TestBad).is_ok());
    }
    assert!(store.report(&addr, Behaviour::TestBad).is_banned());
    assert!(store.is_addr_banned(&addr));
    assert!(store.addr_manager().get(&addr).is_none());
}

// An address with neither an IP nor a peer id must not panic and must not ban anything.
#[test]
fn test_ban_addr_without_ip_or_peer_id_is_a_noop() {
    const ONION: &str = "vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd";
    let addr: Multiaddr = format!("/onion3/{ONION}:1234").parse().unwrap();
    let mut store = PeerStore::default();
    store.ban_addr(&addr, u64::MAX, "test".into());
    assert!(!store.is_addr_banned(&addr));
    assert!(store.ban_list().get_banned_addrs().is_empty());
}

#[test]
fn test_proxy_identity_bans_are_bounded() {
    use crate::peer_store::ban_list::{BanList, MAX_BANNED_PEERS};
    let mut bans = BanList::default();
    let first = random_addr();
    bans.ban_peer(extract_peer_id(&first).unwrap(), u64::MAX);
    for _ in 1..MAX_BANNED_PEERS {
        bans.ban_peer(PeerId::random(), u64::MAX);
    }
    assert!(bans.is_addr_banned(&first));
    let last = random_addr();
    bans.ban_peer(extract_peer_id(&last).unwrap(), u64::MAX);
    assert!(!bans.is_addr_banned(&first));
    assert!(bans.is_addr_banned(&last));
}

#[test]
fn test_add_connected_peer() {
    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    assert_eq!(
        peer_store.fetch_random_addrs(2, Flags::COMPATIBILITY).len(),
        0
    );
    peer_store.add_connected_peer(addr.clone(), SessionType::Outbound);
    peer_store.add_outbound_addr(addr, Flags::COMPATIBILITY);
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
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY, |_| true)
            .len(),
        0
    );
    let addr = random_addr();
    peer_store.add_addr(addr, Flags::COMPATIBILITY).unwrap();
    assert_eq!(peer_store.fetch_addrs_to_feeler(2, |_| true).len(), 1);
    // we have not connected yet, so return 0
    assert_eq!(
        peer_store
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY, |_| true)
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
    assert!(
        peer_store
            .add_addr(addr.clone(), Flags::COMPATIBILITY)
            .is_ok()
    );
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

#[test]
fn test_ban_addr_timeout_saturates_instead_of_overflowing() {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(1_000);

    let mut peer_store: PeerStore = Default::default();
    let addr = random_addr();
    peer_store.add_connected_peer(addr.clone(), SessionType::Inbound);
    // `now_ms + timeout_ms` would overflow u64; it must saturate, not panic.
    peer_store.ban_addr(&addr, u64::MAX, "no reason".into());

    assert!(peer_store.is_addr_banned(&addr));
    assert_eq!(
        peer_store.ban_list().get_banned_addrs()[0].ban_until,
        u64::MAX
    );
}

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
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY, |_| true)
            .len(),
        1
    );
    peer_store.ban_addr(&addr, 10_000, "no reason".into());
    assert_eq!(
        peer_store
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY, |_| true)
            .len(),
        0
    );
}

#[test]
fn test_fetch_addrs_to_attempt() {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(1);

    let mut peer_store: PeerStore = Default::default();
    assert!(
        peer_store
            .fetch_addrs_to_attempt(1, Flags::COMPATIBILITY, |_| true)
            .is_empty()
    );
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
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY, |_| true)
            .len(),
        1
    );
    peer_store.add_connected_peer(addr, SessionType::Outbound);
    assert!(
        peer_store
            .fetch_addrs_to_attempt(1, Flags::COMPATIBILITY, |_| true)
            .is_empty()
    );
}

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
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY, |_| true)
            .len(),
        1
    );
    assert!(peer_store.fetch_addrs_to_feeler(2, |_| true).is_empty());

    _faketime_guard.set_faketime(100_000 + ADDR_TRY_TIMEOUT_MS + 1);

    assert!(
        peer_store
            .fetch_addrs_to_attempt(2, Flags::COMPATIBILITY, |_| true)
            .is_empty()
    );
    assert_eq!(peer_store.fetch_addrs_to_feeler(2, |_| true).len(), 1);
}

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
    assert!(
        peer_store
            .fetch_addrs_to_attempt(1, Flags::COMPATIBILITY, |_| true)
            .is_empty()
    );
    // after 60 seconds
    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&addr) {
        paddr.mark_tried(now - 60_001);
    }
    assert!(
        peer_store
            .fetch_addrs_to_attempt(1, Flags::COMPATIBILITY, |_| true)
            .is_empty()
    );
    peer_store
        .mut_addr_manager()
        .get_mut(&addr)
        .unwrap()
        .mark_connected(now);
    _faketime_guard.set_faketime(200_000);

    assert_eq!(
        peer_store
            .fetch_addrs_to_attempt(1, Flags::COMPATIBILITY, |_| true)
            .len(),
        1
    );
    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&addr) {
        paddr.mark_tried(now);
    }
    assert_eq!(
        peer_store
            .fetch_addrs_to_attempt(1, Flags::COMPATIBILITY, |_| true)
            .len(),
        1
    );
}

#[test]
fn test_fetch_addrs_to_feeler() {
    let mut peer_store: PeerStore = Default::default();
    assert!(peer_store.fetch_addrs_to_feeler(1, |_| true).is_empty());
    let addr = random_addr();

    // add an addr
    peer_store
        .add_addr(addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    assert_eq!(peer_store.fetch_addrs_to_feeler(2, |_| true).len(), 1);

    // ignores connected peers' addrs
    peer_store.add_connected_peer(addr.clone(), SessionType::Outbound);
    assert!(peer_store.fetch_addrs_to_feeler(1, |_| true).is_empty());

    // peer does not need feeler if it connected to us recently
    peer_store
        .mut_addr_manager()
        .get_mut(&addr)
        .unwrap()
        .last_connected_at_ms = ckb_systemtime::unix_time_as_millis();
    peer_store.remove_disconnected_peer(&addr);
    assert!(peer_store.fetch_addrs_to_feeler(1, |_| true).is_empty());
}

#[test]
fn test_fetch_random_addrs() {
    let mut peer_store: PeerStore = Default::default();
    assert!(
        peer_store
            .fetch_random_addrs(1, Flags::COMPATIBILITY)
            .is_empty()
    );
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
    assert!(
        peer_store
            .fetch_random_addrs(1, Flags::COMPATIBILITY)
            .is_empty()
    );
    // can't get peer addr from inbound
    peer_store.add_connected_peer(addr1.clone(), SessionType::Inbound);
    assert!(
        peer_store
            .fetch_random_addrs(1, Flags::COMPATIBILITY)
            .is_empty()
    );
    // get peer addr from outbound
    peer_store.add_connected_peer(addr1.clone(), SessionType::Outbound);
    peer_store.add_outbound_addr(addr1, Flags::COMPATIBILITY);
    assert_eq!(
        peer_store.fetch_random_addrs(2, Flags::COMPATIBILITY).len(),
        1
    );
    // get peer addrs by limit
    peer_store.add_connected_peer(addr2.clone(), SessionType::Outbound);
    peer_store.add_outbound_addr(addr2, Flags::COMPATIBILITY);
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
        2
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
    assert!(
        peer_store
            .fetch_random_addrs(1, Flags::COMPATIBILITY)
            .is_empty()
    );
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
    peer_store.add_outbound_addr(addr1, Flags::COMPATIBILITY);
    peer_store.add_outbound_addr(addr2, Flags::COMPATIBILITY);
    assert_eq!(
        peer_store.fetch_random_addrs(2, Flags::COMPATIBILITY).len(),
        1
    );
}

#[test]
fn test_get_random_with_connected_peer_and_same_peerid() {
    let mut peer_store: PeerStore = Default::default();

    let peer_id = PeerId::random().to_base58();
    let addr1: Multiaddr = format!("/ip4/225.0.0.1/tcp/1867/p2p/{}", peer_id)
        .parse()
        .unwrap();
    let addr2: Multiaddr = format!("/ip4/225.0.0.2/tcp/43/p2p/{}", peer_id)
        .parse()
        .unwrap();

    peer_store
        .add_addr(addr1.clone(), Flags::COMPATIBILITY)
        .unwrap();
    peer_store.add_outbound_addr(addr2, Flags::COMPATIBILITY);

    // Node information that has not been connected must not be selected.
    assert_eq!(
        peer_store.fetch_random_addrs(2, Flags::COMPATIBILITY).len(),
        1
    );

    // add remains connected node info
    peer_store.add_connected_peer(addr1.clone(), SessionType::Outbound);

    // Even if the node remains connected, node's info without connection information cannot be selected.
    assert_eq!(
        peer_store.fetch_random_addrs(2, Flags::COMPATIBILITY).len(),
        1
    );

    peer_store.update_outbound_addr_last_connected_ms(addr1);

    // Set connected info to address, it can be selected
    assert_eq!(
        peer_store.fetch_random_addrs(2, Flags::COMPATIBILITY).len(),
        2
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

#[test]
fn test_addr_unique() {
    let mut peer_store = PeerStore::default();
    let addr = random_addr();
    let addr_1 = random_addr();

    peer_store
        .add_addr(addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    peer_store.add_addr(addr_1, Flags::COMPATIBILITY).unwrap();
    assert_eq!(peer_store.addr_manager().addrs_iter().count(), 2);
    assert_eq!(peer_store.fetch_addrs_to_feeler(2, |_| true).len(), 2);

    peer_store.add_addr(addr, Flags::COMPATIBILITY).unwrap();
    assert_eq!(peer_store.fetch_addrs_to_feeler(2, |_| true).len(), 2);

    assert_eq!(peer_store.addr_manager().addrs_iter().count(), 2);
}

#[test]
fn test_only_tcp_store() {
    let mut peer_store = PeerStore::default();
    let mut addr = random_addr();
    addr.push(p2p::multiaddr::Protocol::Ws);
    peer_store
        .add_addr(addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    assert_eq!(peer_store.fetch_addrs_to_feeler(2, |_| true).len(), 1);
    assert_eq!(peer_store.fetch_addrs_to_feeler(1, |_| true)[0].addr, {
        addr.pop();
        addr
    });
}

#[test]
fn test_support_dns_store() {
    let mut peer_store = PeerStore::default();
    let addr: Multiaddr = format!(
        "/dns4/www.abc.com/tcp/{}/p2p/{}",
        rand::random::<u16>(),
        crate::PeerId::random().to_base58()
    )
    .parse()
    .unwrap();

    peer_store
        .add_addr(addr.clone(), Flags::COMPATIBILITY)
        .unwrap();
    assert_eq!(peer_store.fetch_addrs_to_feeler(2, |_| true).len(), 1);
    assert_eq!(peer_store.fetch_addrs_to_feeler(1, |_| true)[0].addr, addr);
}
