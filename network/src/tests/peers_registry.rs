use crate::{
    memory_peer_store::MemoryPeerStore,
    peer_store::{Behaviour, PeerStore},
    peers_registry::PeersRegistry,
    random_peer_id, PeerId, ToMultiaddr,
};
use ckb_util::RwLock;
use std::default::Default;
use std::sync::Arc;

fn new_peers_registry(
    peer_store: Arc<RwLock<Box<PeerStore>>>,
    max_inbound: u32,
    max_outbound: u32,
    reserved_only: bool,
    reserved_peers: Vec<PeerId>,
) -> PeersRegistry {
    PeersRegistry::new(
        peer_store,
        max_inbound,
        max_outbound,
        reserved_only,
        reserved_peers,
    )
}

#[test]
fn test_accept_inbound_peer_in_reserve_only_mode() {
    let peer_store: Arc<RwLock<Box<PeerStore>>> = Arc::new(RwLock::new(Box::new(
        MemoryPeerStore::new(Default::default()),
    )));
    let reserved_peer = random_peer_id().unwrap();
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();

    // reserved_only mode: only accept reserved_peer
    let mut peers_registry = new_peers_registry(
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
    let peer_store: Arc<RwLock<Box<PeerStore>>> = Arc::new(RwLock::new(Box::new(
        MemoryPeerStore::new(Default::default()),
    )));
    let reserved_peer = random_peer_id().unwrap();
    let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    // accept node until inbound connections is full
    let mut peers_registry = new_peers_registry(
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
    let peer_store: Arc<RwLock<Box<PeerStore>>> = Arc::new(RwLock::new(Box::new(
        MemoryPeerStore::new(Default::default()),
    )));
    let reserved_peer = random_peer_id().unwrap();
    let evict_target = random_peer_id().unwrap();
    let lowest_score_peer = random_peer_id().unwrap();
    let addr1 = "/ip4/127.0.0.1".to_multiaddr().unwrap();
    let addr2 = "/ip4/192.168.0.1".to_multiaddr().unwrap();
    let mut peers_registry = new_peers_registry(
        Arc::clone(&peer_store),
        5,
        3,
        false,
        vec![reserved_peer.clone()],
    );
    // setup 3 node and 1 reserved node from addr1
    peers_registry
        .accept_inbound_peer(reserved_peer.clone(), addr1.clone())
        .expect("accept");
    peers_registry
        .accept_inbound_peer(evict_target.clone(), addr1.clone())
        .expect("accept");
    peers_registry
        .accept_inbound_peer(random_peer_id().unwrap(), addr1.clone())
        .expect("accept");
    peers_registry
        .accept_inbound_peer(random_peer_id().unwrap(), addr1.clone())
        .expect("accept");
    // setup 2 node from addr2
    peers_registry
        .accept_inbound_peer(lowest_score_peer.clone(), addr2.clone())
        .expect("accept");
    peers_registry
        .accept_inbound_peer(random_peer_id().unwrap(), addr2.clone())
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
    // should evict evict target
    assert!(peers_registry.get(&evict_target).is_some());
    peers_registry
        .accept_inbound_peer(random_peer_id().unwrap(), addr1.clone())
        .expect("accept");
    assert!(peers_registry.get(&evict_target).is_none());
}
