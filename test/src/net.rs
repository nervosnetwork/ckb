use crate::specs::TestProtocol;
use crate::Node;
use ckb_network::{
    CKBProtocol, CKBProtocolContext, CKBProtocolHandler, NetworkConfig, NetworkController,
    NetworkService, NetworkState, PeerIndex, ProtocolId,
};
use ckb_types::bytes::Bytes;
use crossbeam_channel::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

pub type NetMessage = (PeerIndex, ProtocolId, Bytes);

pub struct Net {
    pub nodes: Vec<Node>,
    pub controller: Option<(NetworkController, Receiver<NetMessage>)>,
    pub test_protocols: Vec<TestProtocol>,
    num_nodes: usize,
    start_port: u16,
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

        Self {
            nodes,
            controller: None,
            test_protocols,
            start_port,
            num_nodes,
        }
    }

    pub fn connect(&self, node: &Node) {
        if self.controller.is_none() {
            let controller = if self.test_protocols.is_empty() {
                None
            } else {
                let (tx, rx) = crossbeam_channel::unbounded();

                let config = NetworkConfig {
                    listen_addresses: vec![format!("/ip4/127.0.0.1/tcp/{}", self.start_port)
                        .parse()
                        .expect("invalid address")],
                    public_addresses: vec![],
                    bootnodes: vec![],
                    dns_seeds: vec![],
                    whitelist_peers: vec![],
                    whitelist_only: false,
                    max_peers: self.num_nodes as u32,
                    max_outbound_peers: self.num_nodes as u32,
                    path: tempdir()
                        .expect("create tempdir failed")
                        .path()
                        .to_path_buf(),
                    ping_interval_secs: 15,
                    ping_timeout_secs: 20,
                    connect_outbound_interval_secs: 0,
                    discovery_local_address: true,
                    upnp: false,
                    bootnode_mode: false,
                    max_send_buffer: None,
                };

                let network_state =
                    Arc::new(NetworkState::from_config(config).expect("Init network state failed"));

                let protocols = self
                    .test_protocols
                    .clone()
                    .into_iter()
                    .map(|tp| {
                        let tx = tx.clone();
                        CKBProtocol::new(
                            tp.protocol_name,
                            tp.id,
                            &tp.supported_versions,
                            move || Box::new(DummyProtocolHandler { tx: tx.clone() }),
                            Arc::clone(&network_state),
                        )
                    })
                    .collect();

                Some((
                    NetworkService::new(
                        Arc::clone(&network_state),
                        protocols,
                        node.consensus.as_ref().unwrap().identify_name(),
                        "0.1.0".to_string(),
                    )
                    .start(Default::default(), Some("NetworkService"))
                    .expect("Start network service failed"),
                    rx,
                ))
            };

            let ptr = self as *const Self as *mut Self;
            unsafe {
                ::std::mem::replace(&mut (*ptr).controller, controller);
            }
        }

        let node_info = node.rpc_client().local_node_info();
        self.controller.as_ref().unwrap().0.add_node(
            &node_info.node_id.parse().expect("invalid peer_id"),
            format!("/ip4/127.0.0.1/tcp/{}", node.p2p_port())
                .parse()
                .expect("invalid address"),
        );
    }

    /// Blocks the current thread until a message is sent or panic if disconnected
    pub fn send(&self, protocol_id: ProtocolId, peer: PeerIndex, data: Bytes) {
        self.controller
            .as_ref()
            .unwrap()
            .0
            .send_message_to(peer, protocol_id, data)
            .expect("Send message to p2p network failed");
    }

    /// Blocks the current thread until a message is received or panic if disconnected.
    pub fn recv(&self) -> NetMessage {
        self.controller.as_ref().unwrap().1.recv().unwrap()
    }

    /// Waits for a message to be received from the channel, but only for a limited time.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<NetMessage, RecvTimeoutError> {
        self.controller.as_ref().unwrap().1.recv_timeout(timeout)
    }
}

pub struct DummyProtocolHandler {
    tx: Sender<NetMessage>,
}

impl CKBProtocolHandler for DummyProtocolHandler {
    fn init(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>) {}

    fn received(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        data: Bytes,
    ) {
        let _ = self.tx.send((peer_index, nc.protocol_id(), data));
    }
}
