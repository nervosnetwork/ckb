use crate::specs::TestProtocol;
use crate::utils::wait_until;
use crate::Node;
use bytes::Bytes;
use ckb_core::BlockNumber;
use ckb_network::{
    CKBProtocol, CKBProtocolContext, CKBProtocolHandler, NetworkConfig, NetworkController,
    NetworkService, NetworkState, PeerIndex, ProtocolId,
};
use crossbeam_channel::{self, Receiver, RecvTimeoutError, Sender};
use std::collections::HashSet;
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
                    listen_addresses: vec![format!("/ip4/0.0.0.0/tcp/{}", self.start_port)
                        .parse()
                        .expect("invalid address")],
                    public_addresses: vec![],
                    bootnodes: vec![],
                    dns_seeds: vec![],
                    reserved_peers: vec![],
                    reserved_only: false,
                    max_peers: self.num_nodes as u32,
                    max_outbound_peers: self.num_nodes as u32,
                    path: tempdir()
                        .expect("create tempdir failed")
                        .path()
                        .to_path_buf(),
                    ping_interval_secs: 15,
                    ping_timeout_secs: 20,
                    connect_outbound_interval_secs: 1,
                    discovery_local_address: true,
                    upnp: false,
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

    pub fn connect_all(&self) {
        self.nodes
            .windows(2)
            .for_each(|nodes| nodes[0].connect(&nodes[1]));
    }

    pub fn disconnect_all(&self) {
        self.nodes.iter().for_each(|node_a| {
            self.nodes.iter().for_each(|node_b| {
                if node_a.node_id() != node_b.node_id() {
                    node_a.disconnect(node_b)
                }
            })
        });
    }

    pub fn waiting_for_sync(&self, target: BlockNumber) {
        let rpc_clients: Vec<_> = self.nodes.iter().map(Node::rpc_client).collect();
        let mut tip_numbers: HashSet<BlockNumber> = HashSet::with_capacity(self.nodes.len());
        // 60 seconds is a reasonable timeout to sync, even for poor CI server
        let result = wait_until(60, || {
            tip_numbers = rpc_clients
                .iter()
                .map(|rpc_client| rpc_client.get_tip_block_number())
                .collect();
            tip_numbers.len() == 1 && tip_numbers.iter().next().cloned().unwrap() == target
        });

        if !result {
            panic!("timeout to wait for sync, tip_numbers: {:?}", tip_numbers);
        }
    }

    pub fn send(&self, protocol_id: ProtocolId, peer: PeerIndex, data: Bytes) {
        self.controller
            .as_ref()
            .unwrap()
            .0
            .send_message_to(peer, protocol_id, data)
            .expect("Send message to p2p network failed");
    }

    pub fn receive(&self) -> NetMessage {
        self.controller.as_ref().unwrap().1.recv().unwrap()
    }

    pub fn receive_timeout(&self, timeout: Duration) -> Result<NetMessage, RecvTimeoutError> {
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
        data: bytes::Bytes,
    ) {
        let _ = self.tx.send((peer_index, nc.protocol_id(), data));
    }
}
