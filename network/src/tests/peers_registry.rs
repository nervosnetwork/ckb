use crate::{
    memory_peer_store::MemoryPeerStore,
    peer_store::{Behaviour, PeerStore},
    peers_registry::{PeersRegistry, EVICTION_PROTECT_PEERS},
    random_peer_id, ToMultiaddr,
};
use ckb_time::now_ms;
use ckb_util::RwLock;
use std::default::Default;
use std::sync::Arc;

#[test]
fn test_accept_inbound_peer_in_reserve_only_mode() {
    let peer_store: Arc<RwLock<dyn PeerStore>> =
        Arc::new(RwLock::new(MemoryPeerStore::new(Default::default())));
    let reserved_peer = random_peer_id().unwrap();
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();

    // reserved_only mode: only accept reserved_peer
    let mut peers_registry = PeersRegistry::new(
        Arc::clone(&peer_store),
        3,
        3,
        true,
        vec![reserved_peer.clone()],
    );
    assert!(peers_registry
        .accept_inbound_peer(random_peer_id().unwrap(), addr.clone())
        .is_err());
    peers_registry
        .accept_inbound_peer(reserved_peer.clone(), addr.clone())
        .expect("accept");
}

#[test]
fn test_accept_inbound_peer_until_full() {
    let peer_store: Arc<RwLock<dyn PeerStore>> =
        Arc::new(RwLock::new(MemoryPeerStore::new(Default::default())));
    let reserved_peer = random_peer_id().unwrap();
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    // accept node until inbound connections is full
    let mut peers_registry = PeersRegistry::new(
        Arc::clone(&peer_store),
        3,
        3,
        false,
        vec![reserved_peer.clone()],
    );
    peers_registry
        .accept_inbound_peer(random_peer_id().unwrap(), addr.clone())
        .expect("accept");
    peers_registry
        .accept_inbound_peer(random_peer_id().unwrap(), addr.clone())
        .expect("accept");
    peers_registry
        .accept_inbound_peer(random_peer_id().unwrap(), addr.clone())
        .expect("accept");
    assert!(peers_registry
        .accept_inbound_peer(random_peer_id().unwrap(), addr.clone())
        .is_err(),);
    // should still accept reserved peer
    peers_registry
        .accept_inbound_peer(reserved_peer.clone(), addr.clone())
        .expect("accept");
    // should refuse accept low score peer
    assert!(peers_registry
        .accept_inbound_peer(random_peer_id().unwrap(), addr.clone())
        .is_err());
}

#[test]
fn test_accept_inbound_peer_eviction() {
    // eviction inbound peer
    // 1. should evict from largest network groups
    // 2. should never evict reserved peer
    // 3. should evict lowest scored peer
    let peer_store: Arc<RwLock<dyn PeerStore>> =
        Arc::new(RwLock::new(MemoryPeerStore::new(Default::default())));
    let reserved_peer = random_peer_id().unwrap();
    let evict_target = random_peer_id().unwrap();
    let lowest_score_peer = random_peer_id().unwrap();
    let addr1 = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    let addr2 = "/ip4/192.168.0.1".to_multiaddr().unwrap();
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
            .accept_inbound_peer(random_peer_id().unwrap(), addr2.clone())
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
        peer.ping = Some(0);
    }
    // peers which most recently sent messages
    let now = now_ms();
    for _ in 0..EVICTION_PROTECT_PEERS {
        let peer_id = peers_iter.next().unwrap();
        let mut peer = peers_registry.get_mut(&peer_id).unwrap();
        peer.last_message_time = Some(now + 10000);
    }
    // protect 5 peers which have the longest connection time
    for _ in 0..longest_connection_time_peers_count {
        let peer_id = peers_iter.next().unwrap();
        let mut peer = peers_registry.get_mut(&peer_id).unwrap();
        peer.connected_time = Some(now.saturating_sub(10000));
    }
    let mut new_peer_ids = (0..3)
        .into_iter()
        .map(|_| random_peer_id().unwrap())
        .collect::<Vec<_>>();
    // setup 3 node and 1 reserved node from addr1
    peers_registry
        .accept_inbound_peer(reserved_peer.clone(), addr1.clone())
        .expect("accept");
    peers_registry
        .accept_inbound_peer(evict_target.clone(), addr1.clone())
        .expect("accept");
    peers_registry
        .accept_inbound_peer(new_peer_ids[0].clone(), addr1.clone())
        .expect("accept");
    peers_registry
        .accept_inbound_peer(new_peer_ids[1].clone(), addr1.clone())
        .expect("accept");
    // setup 2 node from addr2
    peers_registry
        .accept_inbound_peer(lowest_score_peer.clone(), addr2.clone())
        .expect("accept");
    peers_registry
        .accept_inbound_peer(new_peer_ids[2].clone(), addr2.clone())
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
        peer.connected_time = Some(now + 10000);
    }
    // should evict evict target
    assert!(peers_registry.get(&evict_target).is_some());
    peers_registry
        .accept_inbound_peer(random_peer_id().unwrap(), addr1.clone())
        .expect("accept");
    assert!(peers_registry.get(&evict_target).is_none());
}
