use crate::net_time_checker::{NetTimeChecker, NetTimeProtocol, TOLERANT_OFFSET};
use ckb_app_config::NetworkConfig;
use ckb_network::{
    multiaddr::{Multiaddr, Protocol},
    CKBProtocol, DefaultExitHandler, EventHandler, NetworkState, ServiceBuilder, ServiceControl,
    SessionId, SupportProtocols, TargetProtocol,
};
use futures::StreamExt;
use std::{
    borrow::Cow,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tempfile::tempdir;

#[test]
fn test_samples_collect() {
    let mut ntc = NetTimeChecker::new(3, 5, TOLERANT_OFFSET);
    // zero samples
    assert!(ntc.check().is_ok());
    // 1 sample
    ntc.add_sample(TOLERANT_OFFSET as i64 + 1);
    assert!(ntc.check().is_ok());
    // 3 samples
    ntc.add_sample(TOLERANT_OFFSET as i64 + 2);
    ntc.add_sample(TOLERANT_OFFSET as i64 + 3);
    assert_eq!(ntc.check().unwrap_err(), TOLERANT_OFFSET as i64 + 2);
    // 4 samples
    ntc.add_sample(1);
    assert_eq!(ntc.check().unwrap_err(), TOLERANT_OFFSET as i64 + 1);
    // 5 samples
    ntc.add_sample(2);
    assert_eq!(ntc.check().unwrap_err(), TOLERANT_OFFSET as i64 + 1);
    // 5 samples within tolerant offset
    ntc.add_sample(3);
    ntc.add_sample(4);
    ntc.add_sample(5);
    assert!(ntc.check().is_ok());
    // 5 samples negative offset
    ntc.add_sample(-(TOLERANT_OFFSET as i64) - 1);
    ntc.add_sample(-(TOLERANT_OFFSET as i64) - 2);
    assert!(ntc.check().is_ok());
    ntc.add_sample(-(TOLERANT_OFFSET as i64) - 3);
    assert_eq!(ntc.check().unwrap_err(), -(TOLERANT_OFFSET as i64) - 1);
}

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

    fn session_num(&self) -> usize {
        self.connected_sessions().len()
    }

    fn connected_sessions(&self) -> Vec<SessionId> {
        self.network_state
            .with_peer_registry(|reg| reg.peers().keys().cloned().collect())
    }
}

fn net_service_start() -> Node {
    let config = NetworkConfig {
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
        bootnode_mode: true,
        reuse_port_on_linux: true,
        ..Default::default()
    };

    let network_state =
        Arc::new(NetworkState::from_config(config.clone()).expect("Init network state failed"));

    let net_timer = NetTimeProtocol::default();

    let service_builder = ServiceBuilder::default().insert_protocol(
        CKBProtocol::new_with_support_protocol(
            SupportProtocols::Time,
            Box::new(net_timer),
            Arc::clone(&network_state),
        )
        .build(),
    );

    let mut p2p_service = service_builder
        .key_pair(network_state.local_private_key().clone())
        .upnp(config.upnp)
        .forever(true)
        .build(EventHandler::new(
            Arc::clone(&network_state),
            DefaultExitHandler::default(),
        ));

    let peer_id = network_state.local_peer_id().clone();

    let control = p2p_service.control().clone();
    let (addr_sender, addr_receiver) = ::std::sync::mpsc::channel();

    static RT: once_cell::sync::OnceCell<tokio::runtime::Runtime> =
        once_cell::sync::OnceCell::new();

    let rt = RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap()
    });
    rt.spawn(async move {
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
    });

    let listen_addr = addr_receiver.recv().unwrap();
    Node {
        control,
        listen_addr,
        network_state,
    }
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

#[test]
fn test_protocol() {
    let node1 = net_service_start();
    let node2 = net_service_start();

    node1.dial(
        &node2,
        TargetProtocol::Single(SupportProtocols::Time.protocol_id()),
    );

    wait_connect_state(&node1, 1);
    thread::sleep(Duration::from_secs(5));
}
