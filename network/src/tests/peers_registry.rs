use crate::{
    multiaddr::ToMultiaddr,
    peer_store::{PeerStore, SqlitePeerStore},
    peers_registry::{PeersRegistry, EVICTION_PROTECT_PEERS},
    Behaviour, PeerId, SessionType,
};
use ckb_util::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn new_peer_store() -> Arc<RwLock<dyn PeerStore>> {
    Arc::new(RwLock::new(SqlitePeerStore::temp().expect("temp")))
}

#[test]
fn test_accept_inbound_peer_in_reserve_only_mode() {
    let peer_store = new_peer_store();
    let reserved_peer = PeerId::random();
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    let session_id = 1;
    let session_type = SessionType::Server;

    // reserved_only mode: only accept reserved_peer
    let mut peers = PeersRegistry::new(
        Arc::clone(&peer_store),
        3,
        3,
        true,
        vec![reserved_peer.clone()],
    );
    assert!(peers
        .accept_inbound_peer(PeerId::random(), addr.clone(), session_id, session_type)
        .is_err());
    peers
        .accept_inbound_peer(
            reserved_peer.clone(),
            addr.clone(),
            session_id,
            session_type,
        )
        .expect("accept");
}

#[test]
fn test_accept_inbound_peer_until_full() {
    let peer_store = new_peer_store();
    let reserved_peer = PeerId::random();
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    let session_id = 1;
    let session_type = SessionType::Server;
    // accept node until inbound connections is full
    let mut peers = PeersRegistry::new(
        Arc::clone(&peer_store),
        3,
        3,
        false,
        vec![reserved_peer.clone()],
    );
    peers
        .accept_inbound_peer(PeerId::random(), addr.clone(), session_id, session_type)
        .expect("accept");
    peers
        .accept_inbound_peer(PeerId::random(), addr.clone(), session_id, session_type)
        .expect("accept");
    peers
        .accept_inbound_peer(PeerId::random(), addr.clone(), session_id, session_type)
        .expect("accept");
    println!("{:?}", peers.connection_status());
    assert!(peers
        .accept_inbound_peer(PeerId::random(), addr.clone(), session_id, session_type)
        .is_err(),);
    // should still accept reserved peer
    peers
        .accept_inbound_peer(
            reserved_peer.clone(),
            addr.clone(),
            session_id,
            session_type,
        )
        .expect("accept");
    // should refuse accept low score peer
    assert!(peers
        .accept_inbound_peer(PeerId::random(), addr.clone(), session_id, session_type)
        .is_err());
}

#[test]
fn test_accept_inbound_peer_eviction() {
    // eviction inbound peer
    // 1. should evict from largest network groups
    // 2. should never evict reserved peer
    // 3. should evict lowest scored peer
    let peer_store = new_peer_store();
    let reserved_peer = PeerId::random();
    let evict_target = PeerId::random();
    let lowest_score_peer = PeerId::random();
    let addr1 = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    let addr2 = "/ip4/192.168.0.1".to_multiaddr().unwrap();
    let session_id = 1;
    let session_type = SessionType::Server;
    // prepare protected peers
    let longest_connection_time_peers_count = 5;
    let protected_peers_count = 3 * EVICTION_PROTECT_PEERS + longest_connection_time_peers_count;
    let mut peers_registry = PeersRegistry::new(
        Arc::clone(&peer_store),
        (protected_peers_count + longest_connection_time_peers_count) as u32,
        3,
        false,
        vec![reserved_peer.clone()],
    );
    for _ in 0..protected_peers_count {
        assert!(peers_registry
            .accept_inbound_peer(PeerId::random(), addr2.clone(), session_id, session_type)
            .is_ok());
    }
    let mut peers_iter = peers_registry
        .peers_iter()
        .map(|(peer_id, _)| peer_id.to_owned())
        .collect::<Vec<_>>()
        .into_iter();
    // higest scored peers
    {
        let mut peer_store = peer_store.write();
        for _ in 0..EVICTION_PROTECT_PEERS {
            let peer_id = peers_iter.next().unwrap();
            peer_store.report(&peer_id, Behaviour::Ping);
            peer_store.report(&peer_id, Behaviour::Ping);
        }
    }
    // lowest ping peers
    for _ in 0..EVICTION_PROTECT_PEERS {
        let peer_id = peers_iter.next().unwrap();
        let mut peer = peers_registry.get_mut(&peer_id).unwrap();
        peer.ping = Some(Duration::from_secs(0));
    }

    // to prevent time error, we set now to 10ago.
    let now = Instant::now() - Duration::from_secs(10);
    // peers which most recently sent messages
    for _ in 0..EVICTION_PROTECT_PEERS {
        let peer_id = peers_iter.next().unwrap();
        let mut peer = peers_registry.get_mut(&peer_id).unwrap();
        peer.last_message_time = Some(now + Duration::from_secs(10));
    }
    // protect 5 peers which have the longest connection time
    for _ in 0..longest_connection_time_peers_count {
        let peer_id = peers_iter.next().unwrap();
        let mut peer = peers_registry.get_mut(&peer_id).unwrap();
        peer.connected_time = now - Duration::from_secs(10);
    }
    let mut new_peer_ids = (0..3).map(|_| PeerId::random()).collect::<Vec<_>>();
    // setup 3 node and 1 reserved node from addr1
    peers_registry
        .accept_inbound_peer(
            reserved_peer.clone(),
            addr1.clone(),
            session_id,
            session_type,
        )
        .expect("accept");
    peers_registry
        .accept_inbound_peer(
            evict_target.clone(),
            addr1.clone(),
            session_id,
            session_type,
        )
        .expect("accept");
    peers_registry
        .accept_inbound_peer(
            new_peer_ids[0].clone(),
            addr1.clone(),
            session_id,
            session_type,
        )
        .expect("accept");
    peers_registry
        .accept_inbound_peer(
            new_peer_ids[1].clone(),
            addr1.clone(),
            session_id,
            session_type,
        )
        .expect("accept");
    // setup 2 node from addr2
    peers_registry
        .accept_inbound_peer(
            lowest_score_peer.clone(),
            addr2.clone(),
            session_id,
            session_type,
        )
        .expect("accept");
    peers_registry
        .accept_inbound_peer(
            new_peer_ids[2].clone(),
            addr2.clone(),
            session_id,
            session_type,
        )
        .expect("accept");
    // setup score
    {
        let mut peer_store = peer_store.write();
        peer_store.report(&lowest_score_peer, Behaviour::FailedToPing);
        peer_store.report(&lowest_score_peer, Behaviour::FailedToPing);
        peer_store.report(&lowest_score_peer, Behaviour::FailedToPing);
        peer_store.report(&reserved_peer, Behaviour::FailedToPing);
        peer_store.report(&reserved_peer, Behaviour::FailedToPing);
        peer_store.report(&evict_target, Behaviour::FailedToPing);
    }
    // make sure other peers should not protected by longest connection time rule
    new_peer_ids.extend_from_slice(&[
        reserved_peer.clone(),
        evict_target.clone(),
        lowest_score_peer.clone(),
    ]);
    for peer_id in new_peer_ids {
        let mut peer = peers_registry.get_mut(&peer_id).unwrap();
        // push the connected_time to make sure peer is unprotect
        peer.connected_time = now + Duration::from_secs(10);
    }
    // should evict evict target
    assert!(peers_registry.get(&evict_target).is_some());
    peers_registry
        .accept_inbound_peer(PeerId::random(), addr1.clone(), session_id, session_type)
        .expect("accept");
    assert!(peers_registry.get(&evict_target).is_none());
}
