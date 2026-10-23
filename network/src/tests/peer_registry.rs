#![allow(clippy::unchecked_time_subtraction)]

use super::random_addr;
#[cfg(not(target_family = "wasm"))]
use ckb_app_config::NetworkConfig;

use crate::{
    PeerId, RawSessionType,
    errors::{Error, PeerError},
    extract_peer_id,
    multiaddr::Multiaddr,
    peer_registry::{EVICTION_PROTECT_PEERS, PeerRegistry},
    peer_store::PeerStore,
};
use std::time::{Duration, Instant};

#[cfg(not(target_family = "wasm"))]
fn network_config_with_onion(path: &std::path::Path, listen_on_onion: bool) -> NetworkConfig {
    let mut config = NetworkConfig {
        path: path.to_owned(),
        max_peers: 10,
        max_outbound_peers: 5,
        trusted_proxies: vec!["127.0.0.1".parse().unwrap()],
        ..Default::default()
    };
    config.onion.listen_on_onion = listen_on_onion;
    // The launcher only publishes an onion service when a tor server is reachable.
    config.onion.onion_server = listen_on_onion.then(|| "127.0.0.1:9050".to_string());
    config
}

#[cfg(not(target_family = "wasm"))]
#[tokio::test]
async fn test_onion_proxy_ban_disconnects_only_offending_session() {
    use crate::{NetworkState, network::EventHandler};
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let config = network_config_with_onion(dir.path(), true);
    let state = Arc::new(NetworkState::from_config(config).unwrap());
    let service = p2p::builder::ServiceBuilder::default()
        .handshake_type(state.local_private_key().clone().into())
        .build(EventHandler::new(Arc::clone(&state)));
    let control = service.control().clone().into();
    let offending = random_addr();
    let healthy = random_addr();
    {
        let mut store = state.peer_store.lock();
        let mut registry = state.peer_registry.write();
        for (id, addr) in [(1, offending.clone()), (2, healthy.clone())] {
            registry
                .accept_peer(addr, id.into(), RawSessionType::Inbound, &mut store)
                .unwrap();
        }
    }
    state.ban_session(&control, 1.into(), Duration::from_secs(60), "test".into());
    let mut store = state.peer_store.lock();
    let mut registry = state.peer_registry.write();
    assert!(registry.get_peer(1.into()).is_none());
    assert!(registry.get_peer(2.into()).is_some());
    assert!(store.is_addr_banned(&offending));
    assert!(!store.is_addr_banned(&healthy));
    assert!(store.ban_list().get_banned_addrs().is_empty());
    registry
        .accept_peer(random_addr(), 3.into(), RawSessionType::Inbound, &mut store)
        .expect("new onion peers remain admissible after a session ban");
}

// Without onion listening the connected address is the peer's own address, so banning
// must stay IP based and stay visible to `get_banned_addresses`.
#[cfg(not(target_family = "wasm"))]
#[tokio::test]
async fn test_ban_stays_ip_based_when_onion_listening_is_disabled() {
    use crate::{NetworkState, network::EventHandler};
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let config = network_config_with_onion(dir.path(), false);
    assert!(config.shared_proxy_addrs().is_empty());
    let state = Arc::new(NetworkState::from_config(config).unwrap());
    let service = p2p::builder::ServiceBuilder::default()
        .handshake_type(state.local_private_key().clone().into())
        .build(EventHandler::new(Arc::clone(&state)));
    let control = service.control().clone().into();
    {
        let mut store = state.peer_store.lock();
        let mut registry = state.peer_registry.write();
        registry
            .accept_peer(random_addr(), 1.into(), RawSessionType::Inbound, &mut store)
            .unwrap();
    }
    state.ban_session(&control, 1.into(), Duration::from_secs(60), "test".into());
    let store = state.peer_store.lock();
    let banned = store.ban_list().get_banned_addrs();
    assert_eq!(banned.len(), 1);
    assert_eq!(banned[0].address.to_string(), "127.0.0.1/32");
    // The whole loopback network is banned, exactly as before this change.
    assert!(store.is_addr_banned(&random_addr()));
}

#[test]
fn test_proxy_ban_admission_isolated_by_peer_id() {
    let mut store = PeerStore::default().with_shared_proxy_addrs(&["127.0.0.1".parse().unwrap()]);
    let banned = random_addr();
    let healthy = random_addr();
    let whitelist = random_addr();
    let mut peers = PeerRegistry::new(10, 10, false, vec![whitelist.clone()], true);
    store.ban_addr(&banned, 60_000, "test".into());
    let err = peers
        .accept_peer(banned, 1.into(), RawSessionType::Inbound, &mut store)
        .unwrap_err();
    assert!(matches!(err, Error::Peer(PeerError::Banned)));
    peers
        .accept_peer(
            healthy.clone(),
            2.into(),
            RawSessionType::Inbound,
            &mut store,
        )
        .expect("another proxy client is still admitted");

    store.ban_network(
        crate::peer_store::types::multiaddr_to_ip_network(&healthy).unwrap(),
        60_000,
        "manual".into(),
    );
    let err = peers
        .accept_peer(random_addr(), 3.into(), RawSessionType::Inbound, &mut store)
        .unwrap_err();
    assert!(matches!(err, Error::Peer(PeerError::Banned)));
    peers
        .accept_peer(whitelist, 4.into(), RawSessionType::Inbound, &mut store)
        .expect("whitelisted peers retain their existing exemption");
    assert!(peers.get_peer(2.into()).is_some());
}

#[test]
fn test_accept_inbound_peer_in_reserve_only_mode() {
    let mut peer_store = PeerStore::default();
    let whitelist_addr = format!("/ip4/127.0.0.1/tcp/43/p2p/{}", PeerId::random().to_base58())
        .parse::<Multiaddr>()
        .unwrap();
    let session_id = 1.into();

    // whitelist_only mode: only accept whitelist_peer
    let mut peers = PeerRegistry::new(3, 3, true, vec![whitelist_addr.clone()], true);
    let err = peers
        .accept_peer(
            random_addr(),
            session_id,
            RawSessionType::Inbound,
            &mut peer_store,
        )
        .unwrap_err();
    assert_eq!(
        format!("{err}"),
        format!("{}", Error::Peer(PeerError::NonReserved))
    );

    peers
        .accept_peer(
            whitelist_addr,
            session_id,
            RawSessionType::Inbound,
            &mut peer_store,
        )
        .expect("accept");
}

#[test]
fn test_accept_inbound_peer_until_full() {
    let mut peer_store = PeerStore::default();
    let whitelist_addr = format!("/ip4/127.0.0.1/tcp/43/p2p/{}", PeerId::random().to_base58())
        .parse::<Multiaddr>()
        .unwrap();
    // accept node until inbound connections is full
    let mut peers = PeerRegistry::new(3, 3, false, vec![whitelist_addr.clone()], true);
    for session_id in 1..=3 {
        peers
            .accept_peer(
                random_addr(),
                session_id.into(),
                RawSessionType::Inbound,
                &mut peer_store,
            )
            .expect("accept");
    }

    let err = peers
        .accept_peer(
            random_addr(),
            3.into(),
            RawSessionType::Outbound,
            &mut peer_store,
        )
        .unwrap_err();
    assert_eq!(
        format!("{err}"),
        format!("{}", Error::Peer(PeerError::SessionExists(3.into()))),
    );

    // test evict a peer
    assert!(
        peers
            .accept_peer(
                random_addr(),
                4.into(),
                RawSessionType::Inbound,
                &mut peer_store,
            )
            .expect("Accept peer should ok")
            .is_some()
    );
    // should still accept whitelist peer
    peers
        .accept_peer(
            whitelist_addr.clone(),
            5.into(),
            RawSessionType::Inbound,
            &mut peer_store,
        )
        .expect("accept");
    let err = peers
        .accept_peer(
            whitelist_addr.clone(),
            6.into(),
            RawSessionType::Inbound,
            &mut peer_store,
        )
        .unwrap_err();
    assert_eq!(
        format!("{err}"),
        format!(
            "{}",
            Error::Peer(PeerError::PeerIdExists(
                extract_peer_id(&whitelist_addr).unwrap()
            ))
        ),
    );
}

#[test]
fn test_accept_inbound_peer_eviction() {
    // eviction inbound peer
    // We build an unprotected evict targets set
    // PeerRegistry should
    // 1. evict from largest network groups
    // 2. never evict whitelist peer
    let mut peer_store = PeerStore::default();
    let whitelist_addr = format!("/ip4/127.0.0.1/tcp/43/p2p/{}", PeerId::random().to_base58())
        .parse::<Multiaddr>()
        .unwrap();
    let mut evict_targets = vec![random_addr()];
    // prepare protected peers
    let longest_connection_time_peers_count = 5;
    let protected_peers_count = 2 * (EVICTION_PROTECT_PEERS + longest_connection_time_peers_count);
    let mut peers_registry = PeerRegistry::new(
        (protected_peers_count) as u32,
        3,
        false,
        vec![whitelist_addr],
        true,
    );
    // prepare all peers
    for session_id in 0..protected_peers_count {
        assert!(
            peers_registry
                .accept_peer(
                    random_addr(),
                    session_id.into(),
                    RawSessionType::Inbound,
                    &mut peer_store,
                )
                .is_ok()
        );
    }
    let peers: Vec<_> = {
        peers_registry
            .peers()
            .values()
            .map(|peer| peer.connected_addr.clone())
            .collect()
    };

    let mut peers_iter = peers.iter();
    // lowest ping peers
    for _ in 0..EVICTION_PROTECT_PEERS {
        let peer_addr = peers_iter.next().unwrap();
        let peer_id = extract_peer_id(peer_addr).unwrap();
        let session_id = peers_registry
            .get_key_by_peer_id(&peer_id)
            .expect("get_key_by_peer_id failed");
        if let Some(peer) = peers_registry.get_peer_mut(session_id) {
            peer.ping_rtt = Some(Duration::from_secs(0));
        };
    }

    // to prevent time error, we set now to 10ago.
    let now = Instant::now() - Duration::from_secs(10);
    // peers which most recently sent messages
    for _ in 0..EVICTION_PROTECT_PEERS {
        let peer_addr = peers_iter.next().unwrap();
        let peer_id = extract_peer_id(peer_addr).unwrap();
        let session_id = peers_registry
            .get_key_by_peer_id(&peer_id)
            .expect("get_key_by_peer_id failed");
        if let Some(peer) = peers_registry.get_peer_mut(session_id) {
            peer.last_ping_protocol_message_received_at = Some(now + Duration::from_secs(10));
        };
    }
    // protect half peers which have the longest connection time
    for _ in 0..longest_connection_time_peers_count {
        let peer_addr = peers_iter.next().unwrap();
        let peer_id = extract_peer_id(peer_addr).unwrap();
        let session_id = peers_registry
            .get_key_by_peer_id(&peer_id)
            .expect("get_key_by_peer_id failed");
        if let Some(peer) = peers_registry.get_peer_mut(session_id) {
            peer.connected_time = now - Duration::from_secs(10);
        };
    }
    // these peers will not be protect, we add them to evict_targets
    for _ in 0..longest_connection_time_peers_count {
        let peer_addr = peers_iter.next().unwrap();
        let peer_id = extract_peer_id(peer_addr).unwrap();
        evict_targets.push(peer_addr.to_owned());
        let session_id = peers_registry
            .get_key_by_peer_id(&peer_id)
            .expect("get_key_by_peer_id failed");
        if let Some(peer) = peers_registry.get_peer_mut(session_id) {
            peer.connected_time = now - Duration::from_secs(10);
        };
    }

    peers_registry
        .accept_peer(
            random_addr(),
            2000.into(),
            RawSessionType::Inbound,
            &mut peer_store,
        )
        .expect("accept");
    let len_after_eviction = evict_targets
        .iter()
        .filter_map(|peer_addr| {
            peers_registry.get_key_by_peer_id(&extract_peer_id(peer_addr).unwrap())
        })
        .count();
    // should evict from one of evict_targets
    assert_eq!(len_after_eviction, evict_targets.len() - 1);
}
