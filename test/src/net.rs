use crate::specs::TestProtocol;
use crate::Node;
use bytes::Bytes;
use ckb_network::{
    tokio::runtime::Runtime, CKBProtocol, CKBProtocolContext, CKBProtocolHandler, NetworkConfig,
    NetworkController, NetworkService, NetworkState, ProtocolId, SessionId,
};
use crossbeam_channel::{self, Receiver, Sender};
use tempfile::tempdir;

#[allow(clippy::type_complexity)]
pub struct Net {
    pub nodes: Vec<Node>,
    pub controller: Option<(
        NetworkController,
        Runtime,
        std::thread::JoinHandle<()>,
        Receiver<(SessionId, Bytes)>,
    )>,
}

impl Net {
    pub fn new(
        binary: &str,
        num_nodes: usize,
        start_port: u16,
        test_protocols: Vec<TestProtocol>,
    ) -> Self {
        let nodes: Vec<Node> = (0..num_nodes)
            .map(|n| {
                Node::new(
                    binary,
                    tempdir()
                        .expect("create tempdir failed")
                        .path()
                        .to_str()
                        .unwrap(),
                    start_port + (n * 2 + 1) as u16,
                    start_port + (n * 2 + 2) as u16,
                )
            })
            .collect();

        let controller = if test_protocols.is_empty() {
            None
        } else {
            let (tx, rx) = crossbeam_channel::unbounded();

            let config = NetworkConfig {
                listen_addresses: vec![format!("/ip4/0.0.0.0/tcp/{}", start_port)
                    .parse()
                    .expect("invalid address")],
                public_addresses: vec![],
                bootnodes: vec![],
                reserved_peers: vec![],
                reserved_only: false,
                max_peers: 1,
                max_outbound_peers: 1,
                path: tempdir()
                    .expect("create tempdir failed")
                    .path()
                    .to_path_buf(),
                ping_interval_secs: 15,
                ping_timeout_secs: 20,
                connect_outbound_interval_secs: 1,
            };

            let network_state =
                NetworkState::from_config(config).expect("Init network state failed");

            let protocols = test_protocols
                .into_iter()
                .map(|tp| {
                    let tx = tx.clone();
                    CKBProtocol::new(
                        tp.protocol_name,
                        tp.id,
                        &tp.supported_versions,
                        Box::new(DummyProtocolHandler { tx: tx.clone() }),
                    )
                })
                .collect();

            let (network_service, p2p_service, network_controller) =
                NetworkService::build(network_state, protocols);
            let (network_runtime, network_thread_handle) =
                NetworkService::start(network_service, p2p_service)
                    .expect("Start network service failed");
            Some((
                network_controller,
                network_runtime,
                network_thread_handle,
                rx,
            ))
        };

        Self { nodes, controller }
    }

    pub fn connect(&self, node: &Node) {
        let node_info = node
            .rpc_client()
            .local_node_info()
            .call()
            .expect("rpc call local_node_info failed");
        self.controller.as_ref().unwrap().0.dial_node(
            node_info.node_id.parse().expect("invalid peer_id"),
            format!("/ip4/127.0.0.1/tcp/{}", node.p2p_port)
                .parse()
                .expect("invalid address"),
        );
    }

    pub fn send(&self, protocol_id: ProtocolId, peer: SessionId, data: Vec<u8>) {
        self.controller
            .as_ref()
            .unwrap()
            .0
            .send_message(peer, protocol_id, data);
    }

    pub fn receive(&self) -> (SessionId, Bytes) {
        self.controller.as_ref().unwrap().3.recv().unwrap()
    }
}

pub struct DummyProtocolHandler {
    tx: Sender<(SessionId, Bytes)>,
}

impl CKBProtocolHandler for DummyProtocolHandler {
    fn initialize(&self, _nc: &mut dyn CKBProtocolContext) {}

    fn received(&self, _nc: &mut dyn CKBProtocolContext, peer: SessionId, data: Bytes) {
        let _ = self.tx.send((peer, data));
    }

    fn connected(&self, _nc: &mut dyn CKBProtocolContext, _peer: SessionId) {}

    fn disconnected(&self, _nc: &mut dyn CKBProtocolContext, _peer: SessionId) {}

    fn timer_triggered(&self, _nc: &mut dyn CKBProtocolContext, _timer: u64) {}
}
