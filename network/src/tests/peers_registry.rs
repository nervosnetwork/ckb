use crate::{
    multiaddr::ToMultiaddr,
    peer_store::{PeerStore, SqlitePeerStore},
    peers_registry::{PeersRegistry, EVICTION_PROTECT_PEERS},
    Behaviour, PeerId, ProtocolId, ProtocolVersion, SessionType,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn new_peer_store() -> Arc<dyn PeerStore> {
    Arc::new(SqlitePeerStore::temp().expect("temp"))
}

const TEST_PROTOCOL_ID: ProtocolId = 0;
const TEST_PROTOCOL_VERSION: ProtocolVersion = 0;

#[test]
fn test_accept_inbound_peer_in_reserve_only_mode() {
    let peer_store = new_peer_store();
    let reserved_peer = PeerId::random();
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    let session_id = 1;
    let session_type = SessionType::Inbound;

    // reserved_only mode: only accept reserved_peer
    let peers = PeersRegistry::new(
        Arc::clone(&peer_store),
        3,
        3,
        true,
        vec![reserved_peer.clone()],
    );
    assert!(peers
        .accept_connection(
            PeerId::random(),
            addr.clone(),
            session_id,
            session_type,
            0,
            0
        )
        .is_err());

    peers
        .accept_connection(
            reserved_peer.clone(),
            addr.clone(),
            session_id,
            session_type,
            0,
            0,
        )
        .expect("accept");
}

#[test]
fn test_accept_inbound_peer_until_full() {
    let peer_store = new_peer_store();
    let reserved_peer = PeerId::random();
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    let session_id = 1;
    let session_type = SessionType::Inbound;
    // accept node until inbound connections is full
    let peers = PeersRegistry::new(
        Arc::clone(&peer_store),
        3,
        3,
        false,
        vec![reserved_peer.clone()],
    );
    peers
        .accept_connection(
            PeerId::random(),
            addr.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION,
        )
        .expect("accept");
    peers
        .accept_connection(
            PeerId::random(),
            addr.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION,
        )
        .expect("accept");
    peers
        .accept_connection(
            PeerId::random(),
            addr.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION,
        )
        .expect("accept");
    println!("{:?}", peers.connection_status());
    assert!(peers
        .accept_connection(
            PeerId::random(),
            addr.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION
        )
        .is_err(),);
    // should still accept reserved peer
    peers
        .accept_connection(
            reserved_peer.clone(),
            addr.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION,
        )
        .expect("accept");
    // should refuse accept low score peer
    assert!(peers
        .accept_connection(
            PeerId::random(),
            addr.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION
        )
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
    let session_type = SessionType::Inbound;
    // prepare protected peers
    let longest_connection_time_peers_count = 5;
    let protected_peers_count = 3 * EVICTION_PROTECT_PEERS + longest_connection_time_peers_count;
    let peers_registry = PeersRegistry::new(
        Arc::clone(&peer_store),
        (protected_peers_count + longest_connection_time_peers_count) as u32,
        3,
        false,
        vec![reserved_peer.clone()],
    );
    for _ in 0..protected_peers_count {
        assert!(peers_registry
            .accept_connection(
                PeerId::random(),
                addr2.clone(),
                session_id,
                session_type,
                TEST_PROTOCOL_ID,
                TEST_PROTOCOL_VERSION
            )
            .is_ok());
    }
    let peers: Vec<_> = {
        peers_registry
            .peers_guard()
            .read()
            .iter()
            .map(|(peer_id, _)| peer_id)
            .cloned()
            .collect()
    };

    let mut peers_iter = peers.iter();
    // higest scored peers
    {
        for _ in 0..EVICTION_PROTECT_PEERS {
            let peer_id = peers_iter.next().unwrap();
            peer_store.report(&peer_id, Behaviour::TestGood);
            peer_store.report(&peer_id, Behaviour::TestGood);
        }
    }
    // lowest ping peers
    for _ in 0..EVICTION_PROTECT_PEERS {
        let peer_id = peers_iter.next().unwrap();
        peers_registry.modify_peer(&peer_id, |peer| {
            peer.ping = Some(Duration::from_secs(0));
        });
    }

    // to prevent time error, we set now to 10ago.
    let now = Instant::now() - Duration::from_secs(10);
    // peers which most recently sent messages
    for _ in 0..EVICTION_PROTECT_PEERS {
        let peer_id = peers_iter.next().unwrap();
        peers_registry.modify_peer(&peer_id, |peer| {
            peer.last_message_time = Some(now + Duration::from_secs(10));
        });
    }
    // protect 5 peers which have the longest connection time
    for _ in 0..longest_connection_time_peers_count {
        let peer_id = peers_iter.next().unwrap();
        peers_registry.modify_peer(&peer_id, |peer| {
            peer.connected_time = now - Duration::from_secs(10);
        });
    }
    let mut new_peer_ids = (0..3).map(|_| PeerId::random()).collect::<Vec<_>>();
    // setup 3 node and 1 reserved node from addr1
    peers_registry
        .accept_connection(
            reserved_peer.clone(),
            addr1.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION,
        )
        .expect("accept");
    peers_registry
        .accept_connection(
            evict_target.clone(),
            addr1.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION,
        )
        .expect("accept");
    peers_registry
        .accept_connection(
            new_peer_ids[0].clone(),
            addr1.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION,
        )
        .expect("accept");
    peers_registry
        .accept_connection(
            new_peer_ids[1].clone(),
            addr1.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION,
        )
        .expect("accept");
    // setup 2 node from addr2
    peers_registry
        .accept_connection(
            lowest_score_peer.clone(),
            addr2.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION,
        )
        .expect("accept");
    peers_registry
        .accept_connection(
            new_peer_ids[2].clone(),
            addr2.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION,
        )
        .expect("accept");
    // setup score
    {
        peer_store.report(&lowest_score_peer, Behaviour::TestBad);
        peer_store.report(&lowest_score_peer, Behaviour::TestBad);
        peer_store.report(&lowest_score_peer, Behaviour::TestBad);
        peer_store.report(&reserved_peer, Behaviour::TestBad);
        peer_store.report(&reserved_peer, Behaviour::TestBad);
        peer_store.report(&evict_target, Behaviour::TestBad);
    }
    // make sure other peers should not protected by longest connection time rule
    new_peer_ids.extend_from_slice(&[
        reserved_peer.clone(),
        evict_target.clone(),
        lowest_score_peer.clone(),
    ]);
    for peer_id in new_peer_ids {
        peers_registry.modify_peer(&peer_id, |peer| {
            // push the connected_time to make sure peer is unprotect
            peer.connected_time = now + Duration::from_secs(10);
        });
    }
    // should evict evict target
    assert!(peers_registry
        .peers_guard()
        .read()
        .get(&evict_target)
        .is_some());
    peers_registry
        .accept_connection(
            PeerId::random(),
            addr1.clone(),
            session_id,
            session_type,
            TEST_PROTOCOL_ID,
            TEST_PROTOCOL_VERSION,
        )
        .expect("accept");
    assert!(peers_registry
        .peers_guard()
        .read()
        .get(&evict_target)
        .is_none());
}
