use crate::{
    peer_store::{
        db_trace,
        sqlite_peer_store::{PEER_NOT_SEEN_TIMEOUT_SECS, PEER_STORE_LIMIT},
        Behaviour, PeerStore, SqlitePeerStore, Status,
    },
    random_peer_id, Endpoint, ToMultiaddr,
};
use std::time::Duration;

#[test]
fn test_new_connected_peer() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(SqlitePeerStore::default());
    let peer_id = random_peer_id().unwrap();
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    peer_store.new_connected_peer(&peer_id, addr, Endpoint::Dialer);
    assert_eq!(
        peer_store.peer_score(&peer_id).unwrap(),
        peer_store.scoring_schema().peer_init_score()
    );
    assert_eq!(peer_store.peer_addrs(&peer_id, 1).unwrap().len(), 0);
}

#[test]
fn test_add_discovered_address() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(SqlitePeerStore::default());
    let peer_id = random_peer_id().unwrap();
    peer_store
        .add_discovered_address(&peer_id, "/ip4/127.0.0.1".to_multiaddr().unwrap())
        .expect("add discovered address");
    assert_eq!(peer_store.peer_addrs(&peer_id, 2).unwrap().len(), 1);
    peer_store
        .add_discovered_addresses(
            &peer_id,
            vec![
                "/ip4/127.0.0.1".to_multiaddr().unwrap(),
                "/ip4/192.168.2.2".to_multiaddr().unwrap(),
                "/ip4/192.168.2.3".to_multiaddr().unwrap(),
            ],
        )
        .expect("add discovered address");
    assert_eq!(peer_store.peer_addrs(&peer_id, 4).unwrap().len(), 3);
}

#[test]
fn test_report() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(SqlitePeerStore::default());
    let peer_id = random_peer_id().unwrap();
    assert!(peer_store.report(&peer_id, Behaviour::Ping).is_ok());
    assert!(
        peer_store.peer_score_or_default(&peer_id) > peer_store.scoring_schema().peer_init_score()
    );
}

#[test]
fn test_update_status() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(SqlitePeerStore::default());
    let peer_id = random_peer_id().unwrap();
    peer_store.update_status(&peer_id, Status::Connected);
    assert_eq!(peer_store.peer_status(&peer_id), Status::Unknown);
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    peer_store.new_connected_peer(&peer_id, addr, Endpoint::Listener);
    peer_store.update_status(&peer_id, Status::Connected);
    assert_eq!(peer_store.peer_status(&peer_id), Status::Connected);
}

#[test]
fn test_ban_peer() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(SqlitePeerStore::default());
    let peer_id = random_peer_id().unwrap();
    peer_store.ban_peer(&peer_id, Duration::from_secs(10));
    assert!(!peer_store.is_banned(&peer_id));
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    peer_store.new_connected_peer(&peer_id, addr, Endpoint::Listener);
    peer_store.ban_peer(&peer_id, Duration::from_secs(10));
    assert!(peer_store.is_banned(&peer_id));
}

#[test]
fn test_bootnodes() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(SqlitePeerStore::default());
    assert!(peer_store.bootnodes(1).is_empty());
    let peer_id = random_peer_id().unwrap();
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    peer_store.add_bootnode(peer_id.clone(), addr.clone());
    assert_eq!(peer_store.bootnodes(2).len(), 1);
    let peer_id2 = random_peer_id().unwrap();
    peer_store
        .add_discovered_address(&peer_id2, addr.clone())
        .expect("add discovered address");
    assert_eq!(
        peer_store.bootnodes(3),
        vec![(peer_id2, addr.clone()), (peer_id, addr)]
    );
}

#[test]
fn test_peers_to_attempt() {
    let mut peer_store: Box<dyn PeerStore> = Box::new(SqlitePeerStore::default());
    assert!(peer_store.peers_to_attempt(1).is_empty());
    let peer_id = random_peer_id().unwrap();
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    peer_store.add_bootnode(peer_id.clone(), addr.clone());
    assert!(peer_store.peers_to_attempt(1).is_empty());
    let peer_id2 = random_peer_id().unwrap();
    peer_store
        .add_discovered_address(&peer_id2, addr.clone())
        .expect("add discovered address");
    assert_eq!(peer_store.peers_to_attempt(2).len(), 1);
    peer_store.update_status(&peer_id2, Status::Connected);
    assert!(peer_store.peers_to_attempt(1).is_empty());
}

#[test]
fn test_delete_peer_info() {
    let mut peer_store = SqlitePeerStore::default();
    let addr1 = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    let addr2 = "/ip4/192.163.1.1".to_multiaddr().unwrap();
    {
        let mut conn = peer_store.connection().lock();
        db_trace::start_profile(&mut conn);
    }
    for _ in 0..(PEER_STORE_LIMIT - 2) {
        peer_store.new_connected_peer(
            &random_peer_id().unwrap(),
            addr1.clone(),
            Endpoint::Listener,
        );
    }
    let evict_target = random_peer_id().unwrap();
    let fake_target = random_peer_id().unwrap();
    {
        // make sure these 2 peers become candidate in eviction
        let recent_not_seen_time =
            faketime::unix_time() - Duration::from_secs((PEER_NOT_SEEN_TIMEOUT_SECS + 1) as u64);
        let faketime_file = faketime::millis_tempfile(recent_not_seen_time.as_secs() * 1000)
            .expect("create faketime file");
        faketime::enable(&faketime_file);
        peer_store.new_connected_peer(&evict_target, addr1.clone(), Endpoint::Listener);
        peer_store.new_connected_peer(&fake_target, addr2, Endpoint::Listener);
    }
    peer_store.report(&evict_target, Behaviour::FailedToPing);
    peer_store.report(&fake_target, Behaviour::FailedToPing);
    peer_store.report(&fake_target, Behaviour::FailedToPing);
    // evict_target has lower score than init score
    assert!(
        peer_store.peer_score_or_default(&evict_target)
            < peer_store.scoring_schema().peer_init_score()
    );
    // should evict evict_target and accept this
    peer_store.new_connected_peer(&random_peer_id().unwrap(), addr1, Endpoint::Listener);
    {
        let mut conn = peer_store.connection().lock();
        db_trace::stop_profile(&mut conn);
    }
    let profile_result = db_trace::PROFILE_INFORMATION.lock().clone();
    println!("profile result: {:?}", profile_result);
    // evict_target is evicted in previous step
    assert_eq!(
        peer_store.peer_score_or_default(&evict_target),
        peer_store.scoring_schema().peer_init_score()
    );
}
