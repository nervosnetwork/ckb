use super::{
    discovery::{DiscoveryAddressManager, DiscoveryProtocol},
    feeler::Feeler,
    identify::{IdentifyCallback, IdentifyProtocol},
    ping::PingHandler,
};

use crate::{
    network::{DefaultExitHandler, EventHandler},
    NetworkState, PeerIdentifyInfo, SupportProtocols,
};

use std::{
    borrow::Cow,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use ckb_app_config::NetworkConfig;
use futures::StreamExt;
use p2p::{
    builder::ServiceBuilder,
    multiaddr::{Multiaddr, Protocol},
    service::{ProtocolHandle, ServiceControl, TargetProtocol},
    utils::multiaddr_to_socketaddr,
    ProtocolId, SessionId,
};
use tempfile::tempdir;

struct Node {
    listen_addr: Multiaddr,
    control: ServiceControl,
    network_state: Arc<NetworkState>,
}

impl Node {
    fn dial(&self, node: &Node, protocol: TargetProtocol) {
        self.control
            .dial(node.listen_addr.clone(), protocol)
            .unwrap();
    }

    fn dial_addr(&self, addr: Multiaddr, protocol: TargetProtocol) {
        self.control.dial(addr, protocol).unwrap();
    }

    fn session_num(&self) -> usize {
        self.connected_sessions().len()
    }

    fn connected_sessions(&self) -> Vec<SessionId> {
        self.network_state
            .peer_registry
            .read()
            .peers()
            .keys()
            .cloned()
            .collect()
    }

    fn connected_protocols(&self, id: SessionId) -> Vec<ProtocolId> {
        self.network_state
            .peer_registry
            .read()
            .peers()
            .get(&id)
            .map(|peer| peer.protocols.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn session_version(&self, id: SessionId) -> Option<PeerIdentifyInfo> {
        self.network_state
            .peer_registry
            .read()
            .peers()
            .get(&id)
            .map(|peer| peer.identify_info.clone())
            .unwrap_or_default()
    }

    fn open_protocols(&self, id: SessionId, protocol: TargetProtocol) {
        self.control.open_protocols(id, protocol).unwrap();
    }

    fn ban_all(&self) {
        for id in self.connected_sessions() {
            self.network_state.ban_session(
                &self.control,
                id,
                Duration::from_secs(20),
                Default::default(),
            );
        }
    }
}

fn net_service_start(name: String) -> Node {
    let config = NetworkConfig {
        listen_addresses: vec![],
        public_addresses: vec![],
        bootnodes: vec![],
        dns_seeds: vec![],
        whitelist_peers: vec![],
        whitelist_only: false,
        max_peers: 19,
        max_outbound_peers: 5,
        path: tempdir()
            .expect("create tempdir failed")
            .path()
            .to_path_buf(),
        ping_interval_secs: 15,
        ping_timeout_secs: 20,
        connect_outbound_interval_secs: 1,
        discovery_local_address: true,
        upnp: false,
        bootnode_mode: true,
        max_send_buffer: None,
        sync: None,
    };

    let network_state =
        Arc::new(NetworkState::from_config(config.clone()).expect("Init network state failed"));

    network_state.protocols.write().push((
        SupportProtocols::Ping.protocol_id(),
        SupportProtocols::Ping.name(),
        SupportProtocols::Ping.support_versions(),
    ));
    network_state.protocols.write().push((
        SupportProtocols::Discovery.protocol_id(),
        SupportProtocols::Discovery.name(),
        SupportProtocols::Discovery.support_versions(),
    ));
    network_state.protocols.write().push((
        SupportProtocols::Identify.protocol_id(),
        SupportProtocols::Identify.name(),
        SupportProtocols::Identify.support_versions(),
    ));
    network_state.protocols.write().push((
        SupportProtocols::Feeler.protocol_id(),
        SupportProtocols::Feeler.name(),
        SupportProtocols::Feeler.support_versions(),
    ));

    // Ping protocol
    let ping_interval = Duration::from_secs(5);
    let ping_timeout = Duration::from_secs(10);

    let ping_network_state = Arc::clone(&network_state);
    let (ping_handler, _ping_controller) =
        PingHandler::new(ping_interval, ping_timeout, ping_network_state);
    let ping_meta = SupportProtocols::Ping
        .build_meta_with_service_handle(move || ProtocolHandle::Callback(Box::new(ping_handler)));

    // Discovery protocol
    let addr_mgr = DiscoveryAddressManager {
        network_state: Arc::clone(&network_state),
        discovery_local_address: config.discovery_local_address,
    };
    let disc_meta = SupportProtocols::Discovery.build_meta_with_service_handle(move || {
        ProtocolHandle::Callback(Box::new(DiscoveryProtocol::new(addr_mgr)))
    });

    // Identify protocol
    let identify_callback =
        IdentifyCallback::new(Arc::clone(&network_state), name, "0.1.0".to_string());
    let identify_meta = SupportProtocols::Identify.build_meta_with_service_handle(move || {
        ProtocolHandle::Callback(Box::new(IdentifyProtocol::new(identify_callback)))
    });

    // Feeler protocol
    let feeler_meta = SupportProtocols::Feeler.build_meta_with_service_handle({
        let network_state = Arc::clone(&network_state);
        move || ProtocolHandle::Both(Box::new(Feeler::new(Arc::clone(&network_state))))
    });

    let service_builder = ServiceBuilder::default()
        .insert_protocol(ping_meta)
        .insert_protocol(disc_meta)
        .insert_protocol(identify_meta)
        .insert_protocol(feeler_meta);

    let mut p2p_service = service_builder
        .key_pair(network_state.local_private_key().clone())
        .upnp(config.upnp)
        .forever(true)
        .build(EventHandler {
            network_state: Arc::clone(&network_state),
            exit_handler: DefaultExitHandler::default(),
        });

    let peer_id = network_state.local_peer_id().clone();

    let control = p2p_service.control().clone();
    let (addr_sender, addr_receiver) = ::std::sync::mpsc::channel();

    thread::spawn(move || {
        let num_threads = ::std::cmp::max(num_cpus::get(), 4);
        let mut rt = tokio::runtime::Builder::new()
            .core_threads(num_threads)
            .enable_all()
            .threaded_scheduler()
            .build()
            .unwrap();
        rt.block_on(async move {
            let mut listen_addr = p2p_service
                .listen("/ip4/127.0.0.1/tcp/0".parse().unwrap())
                .await
                .unwrap();
            listen_addr.push(Protocol::P2P(Cow::Owned(peer_id.into_bytes())));
            addr_sender.send(listen_addr).unwrap();
            loop {
                if p2p_service.next().await.is_none() {
                    break;
                }
            }
        })
    });

    let listen_addr = addr_receiver.recv().unwrap();
    Node {
        control,
        listen_addr,
        network_state,
    }
}

pub fn wait_until<F>(secs: u64, f: F) -> bool
where
    F: Fn() -> bool,
{
    let start = Instant::now();
    let timeout = Duration::new(secs, 0);
    while Instant::now().duration_since(start) <= timeout {
        if f() {
            return true;
        }
        thread::sleep(Duration::new(1, 0));
    }
    false
}

fn wait_connect_state(node: &Node, expect_num: usize) {
    if !wait_until(10, || node.session_num() == expect_num) {
        panic!(
            "node session number is {}, not {}",
            node.session_num(),
            expect_num
        )
    }
}

#[allow(clippy::blocks_in_if_conditions)]
fn wait_discovery(node: &Node) {
    if !wait_until(100, || {
        node.network_state
            .peer_store
            .lock()
            .mut_addr_manager()
            .count()
            >= 2
    }) {
        panic!("discovery can't find other node")
    }
}

#[test]
fn test_identify_behavior() {
    let node1 = net_service_start("/test/1".to_string());
    let node2 = net_service_start("/test/2".to_string());
    let node3 = net_service_start("/test/1".to_string());

    node1.dial(
        &node3,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );

    wait_connect_state(&node1, 1);
    wait_connect_state(&node3, 1);

    // identify will ban node when they are on the different net
    node2.dial(
        &node3,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );

    wait_connect_state(&node2, 0);
    wait_connect_state(&node3, 1);

    node1.dial(
        &node2,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );

    wait_connect_state(&node1, 1);
    wait_connect_state(&node2, 0);

    let sessions = node3.connected_sessions();
    assert_eq!(sessions.len(), 1);

    if !wait_until(10, || node3.connected_protocols(sessions[0]).len() == 3) {
        panic!("identify can't open other protocols")
    }

    assert_eq!(
        node3.session_version(sessions[0]).unwrap().client_version,
        "0.1.0"
    );

    let mut protocols = node3.connected_protocols(sessions[0]);
    protocols.sort();

    assert_eq!(
        protocols,
        vec![
            SupportProtocols::Ping.protocol_id(),
            SupportProtocols::Discovery.protocol_id(),
            SupportProtocols::Identify.protocol_id()
        ]
    );
}

#[test]
fn test_feeler_behavior() {
    let node1 = net_service_start("/test/1".to_string());
    let node2 = net_service_start("/test/1".to_string());

    node1.dial(
        &node2,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );

    wait_connect_state(&node1, 1);
    wait_connect_state(&node2, 1);

    node2.open_protocols(
        node2.connected_sessions()[0],
        TargetProtocol::Single(SupportProtocols::Feeler.protocol_id()),
    );

    wait_connect_state(&node1, 0);
    wait_connect_state(&node2, 0);
}

#[test]
fn test_discovery_behavior() {
    let node1 = net_service_start("/test/1".to_string());
    let node2 = net_service_start("/test/1".to_string());
    let node3 = net_service_start("/test/1".to_string());

    node1.dial(
        &node2,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );
    node3.dial(
        &node2,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );

    wait_connect_state(&node1, 1);
    wait_connect_state(&node2, 2);
    wait_connect_state(&node3, 1);

    wait_discovery(&node3);

    let addr = {
        let listen_addr = &node3.listen_addr;
        node3
            .network_state
            .peer_store
            .lock()
            .fetch_addrs_to_attempt(2)
            .into_iter()
            .map(|peer| peer.addr)
            .find(|addr| {
                match (
                    multiaddr_to_socketaddr(&addr),
                    multiaddr_to_socketaddr(listen_addr),
                ) {
                    (Some(dis), Some(listen)) => dis.port() != listen.port(),
                    _ => false,
                }
            })
            .unwrap()
    };

    node3.dial_addr(
        addr,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );

    wait_connect_state(&node1, 2);
    wait_connect_state(&node2, 2);
    wait_connect_state(&node3, 2);
}

#[test]
fn test_dial_all() {
    let node1 = net_service_start("/test/1".to_string());
    let node2 = net_service_start("/test/1".to_string());

    node1.dial(&node2, TargetProtocol::All);

    wait_connect_state(&node1, 0);
    wait_connect_state(&node1, 0);
}

#[test]
fn test_ban() {
    let node1 = net_service_start("/test/1".to_string());
    let node2 = net_service_start("/test/1".to_string());

    node1.dial(
        &node2,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );

    wait_connect_state(&node1, 1);
    wait_connect_state(&node2, 1);

    node1.ban_all();

    wait_connect_state(&node1, 0);
    wait_connect_state(&node2, 0);

    node1.dial(
        &node2,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );
    node1.dial(
        &node2,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );
    node1.dial(
        &node2,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );
    node1.dial(
        &node2,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );

    wait_connect_state(&node1, 0);
    wait_connect_state(&node2, 0);
}

#[test]
fn test_bootnode_mode_inbound_eviction() {
    let node1 = net_service_start("/test/1".to_string());
    let node2 = net_service_start("/test/1".to_string());
    let node3 = net_service_start("/test/1".to_string());
    let node4 = net_service_start("/test/1".to_string());
    let node5 = net_service_start("/test/1".to_string());
    let node6 = net_service_start("/test/1".to_string());

    node2.dial(
        &node1,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );
    node3.dial(
        &node1,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );
    node4.dial(
        &node1,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );

    // Normal connection
    wait_connect_state(&node1, 3);
    node5.dial(
        &node1,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );

    wait_connect_state(&node1, 4);
    // Arrival eviction condition 4 + 10, eviction 2
    node6.dial(
        &node1,
        TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
    );

    // Normal connection, 2 + 1
    wait_connect_state(&node1, 3);
}
