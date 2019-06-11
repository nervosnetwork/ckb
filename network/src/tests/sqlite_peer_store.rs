use crate::{
    multiaddr::Multiaddr,
    peer_store::{
        sqlite::db, types::PeerInfo, PeerStore, SqlitePeerStore, Status, ADDR_TIMEOUT_MS,
        PEER_STORE_LIMIT,
    },
    Behaviour, PeerId, SessionType,
};
use std::time::Duration;

fn new_peer_store() -> SqlitePeerStore {
    SqlitePeerStore::memory().expect("memory")
}

#[test]
fn test_add_connected_peer() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(new_peer_store());
    let peer_id = PeerId::random();
    let addr = "/ip4/127.0.0.1".parse().unwrap();
    peer_store.add_connected_peer(&peer_id, addr, SessionType::Outbound);
    assert_eq!(peer_store.peer_addrs(&peer_id, 1).len(), 1);
}

#[test]
fn test_add_discovered_addr() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(new_peer_store());
    let peer_id = PeerId::random();
    peer_store.add_discovered_addr(&peer_id, "/ip4/127.0.0.1".parse().unwrap());
    assert_eq!(peer_store.peer_addrs(&peer_id, 2).len(), 1);
}

#[test]
fn test_report() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(new_peer_store());
    let peer_id = PeerId::random();
    assert!(peer_store.report(&peer_id, Behaviour::TestGood).is_ok());
}

#[test]
fn test_update_status() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(new_peer_store());
    let peer_id = PeerId::random();
    peer_store.update_status(&peer_id, Status::Connected);
    assert_eq!(peer_store.peer_status(&peer_id), Status::Unknown);
    let addr = "/ip4/127.0.0.1".parse().unwrap();
    peer_store.add_connected_peer(&peer_id, addr, SessionType::Inbound);
    peer_store.update_status(&peer_id, Status::Connected);
    assert_eq!(peer_store.peer_status(&peer_id), Status::Connected);
}

#[test]
fn test_ban_peer() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(new_peer_store());
    let peer_id = PeerId::random();
    let addr: Multiaddr = "/ip4/127.0.0.1".parse().unwrap();
    peer_store.add_connected_peer(&peer_id, addr.clone(), SessionType::Inbound);
    peer_store.ban_addr(&addr, Duration::from_secs(10));
    assert!(peer_store.is_banned(&addr));
}

#[test]
fn test_attempt_ban() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(new_peer_store());
    let peer_id = PeerId::random();
    let addr = "/ip4/127.0.0.1".parse::<Multiaddr>().unwrap();
    peer_store.add_connected_peer(&peer_id, addr.clone(), SessionType::Inbound);
    peer_store.add_discovered_addr(&peer_id, addr.clone());
    assert_eq!(peer_store.peers_to_attempt(2).len(), 1);
    peer_store.ban_addr(&addr, Duration::from_secs(10));
    assert_eq!(peer_store.peers_to_attempt(2).len(), 0);
}

#[test]
fn test_bootnodes() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(new_peer_store());
    assert!(peer_store.bootnodes(1).is_empty());
    let peer_id = PeerId::random();
    let addr = "/ip4/127.0.0.1".parse::<Multiaddr>().unwrap();
    peer_store.add_bootnode(peer_id.clone(), addr.clone());
    assert_eq!(peer_store.bootnodes(2).len(), 1);
    let peer_id2 = PeerId::random();
    peer_store.add_discovered_addr(&peer_id2, addr.clone());
    assert_eq!(
        peer_store.bootnodes(3),
        vec![(peer_id2, addr.clone()), (peer_id, addr)]
    );
}

#[test]
fn test_peers_to_attempt() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(new_peer_store());
    assert!(peer_store.peers_to_attempt(1).is_empty());
    let peer_id = PeerId::random();
    let addr = "/ip4/127.0.0.1".parse::<Multiaddr>().unwrap();
    peer_store.add_bootnode(peer_id.clone(), addr.clone());
    assert!(peer_store.peers_to_attempt(1).is_empty());
    let peer_id2 = PeerId::random();
    peer_store.add_discovered_addr(&peer_id2, addr.clone());
    assert_eq!(peer_store.peers_to_attempt(2).len(), 1);
    peer_store.update_status(&peer_id2, Status::Connected);
    assert!(peer_store.peers_to_attempt(1).is_empty());
}

#[test]
fn test_peers_to_feeler() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(new_peer_store());
    assert!(peer_store.peers_to_feeler(1).is_empty());
    let peer_id = PeerId::random();
    let addr = "/ip4/127.0.0.1".parse::<Multiaddr>().unwrap();
    peer_store.add_bootnode(peer_id.clone(), addr.clone());
    assert!(peer_store.peers_to_feeler(1).is_empty());
    let peer_id2 = PeerId::random();
    peer_store.add_discovered_addr(&peer_id2, addr.clone());
    assert_eq!(peer_store.peers_to_feeler(2).len(), 1);
    peer_store.update_status(&peer_id2, Status::Connected);
    assert!(peer_store.peers_to_feeler(1).is_empty());
    peer_store.update_status(&peer_id2, Status::Unknown);
    assert_eq!(peer_store.peers_to_feeler(2).len(), 1);
    // peer does not need feeler if it connected to us recently
    peer_store.add_connected_peer(&peer_id2, addr.clone(), SessionType::Inbound);
    peer_store.update_status(&peer_id2, Status::Unknown);
    assert!(peer_store.peers_to_feeler(1).is_empty());
}

#[test]
fn test_random_peers() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(new_peer_store());
    assert!(peer_store.random_peers(1).is_empty());
    let peer_id = PeerId::random();
    let addr = "/ip4/127.0.0.1".parse::<Multiaddr>().unwrap();
    peer_store.add_bootnode(peer_id.clone(), addr.clone());
    assert!(peer_store.random_peers(1).is_empty());
    let peer_id2 = PeerId::random();
    peer_store.add_discovered_addr(&peer_id2, addr.clone());
    // random should not return peer that we have never connected to
    assert!(peer_store.random_peers(1).is_empty());
    peer_store.add_connected_peer(&peer_id2, addr.clone(), SessionType::Inbound);
    assert_eq!(peer_store.random_peers(2).len(), 1);
    peer_store.update_status(&peer_id2, Status::Connected);
    assert_eq!(peer_store.random_peers(1).len(), 1);
}

#[test]
fn test_delete_peer_info() {
    let mut peer_store = new_peer_store();
    let addr1: Multiaddr = "/ip4/127.0.0.1".parse().unwrap();
    let addr2: Multiaddr = "/ip4/192.163.1.1".parse().unwrap();
    let now = faketime::unix_time_as_millis();
    // prepare peer_info records
    for _ in 0..(PEER_STORE_LIMIT - 2) {
        db::PeerInfoDB::insert_or_update(
            &peer_store.conn,
            &PeerInfo::new(
                PeerId::random(),
                addr1.clone(),
                peer_store.peer_score_config().default_score,
                SessionType::Inbound,
                now,
            ),
        )
        .expect("insert peer infos");
    }
    let evict_target = PeerId::random();
    let fake_target = PeerId::random();
    {
        // make sure these 2 peers become candidate in eviction
        let recent_not_seen_time = faketime::unix_time() - Duration::from_secs(ADDR_TIMEOUT_MS + 1);
        let faketime_file = faketime::millis_tempfile(recent_not_seen_time.as_secs() * 1000)
            .expect("create faketime file");
        faketime::enable(&faketime_file);
        peer_store.add_connected_peer(&evict_target, addr1.clone(), SessionType::Inbound);
        peer_store.add_connected_peer(&fake_target, addr2, SessionType::Inbound);
    }
    peer_store.report(&evict_target, Behaviour::TestBad);
    peer_store.report(&fake_target, Behaviour::TestBad);
    peer_store.report(&fake_target, Behaviour::TestBad);
    // should evict evict_target and accept this
    peer_store.add_connected_peer(&PeerId::random(), addr1, SessionType::Inbound);
    // evict_target is evicted in previous step
    assert!(
        db::PeerInfoDB::get_by_peer_id(&peer_store.conn, &evict_target)
            .expect("get peer")
            .is_none()
    );
}
