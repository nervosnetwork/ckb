use crate::specs::TestProtocol;
use crate::{sleep, Node};
use bytes::Bytes;
use ckb_core::BlockNumber;
use ckb_network::{
    CKBProtocol, CKBProtocolContext, CKBProtocolHandler, NetworkConfig, NetworkController,
    NetworkService, NetworkState, PeerIndex, ProtocolId,
};
use crossbeam_channel::{self, Receiver, Sender};
use std::collections::HashSet;
use std::sync::Arc;
use tempfile::tempdir;

pub type NetMessage = (PeerIndex, ProtocolId, Bytes);

pub struct Net {
    pub nodes: Vec<Node>,
    pub controller: Option<(NetworkController, Receiver<NetMessage>)>,
}

impl Net {
    pub fn new(
        binary: &str,
        num_nodes: usize,
        start_port: u16,
        test_protocols: Vec<TestProtocol>,
        cellbase_maturity: Option<BlockNumber>,
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
                    cellbase_maturity,
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
                dns_seeds: vec![],
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
                Arc::new(NetworkState::from_config(config).expect("Init network state failed"));

            let protocols = test_protocols
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
                NetworkService::new(Arc::clone(&network_state), protocols)
                    .start(Default::default(), Some("NetworkService"))
                    .expect("Start network service failed"),
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
        self.controller.as_ref().unwrap().0.add_node(
            &node_info.node_id.parse().expect("invalid peer_id"),
            format!("/ip4/127.0.0.1/tcp/{}", node.p2p_port)
                .parse()
                .expect("invalid address"),
        );
    }

    pub fn waiting_for_sync(&self, timeout: u64) {
        for _ in 0..timeout {
            sleep(1);
            let tip_numbers: HashSet<_> = self
                .nodes
                .iter()
                .map(|node| {
                    node.rpc_client()
                        .get_tip_block_number()
                        .call()
                        .expect("rpc call get_tip_block_number failed")
                })
                .collect();
            if tip_numbers.len() == 1 {
                return;
            }
        }
        panic!("Waiting for sync timeout");
    }

    pub fn send(&self, protocol_id: ProtocolId, peer: PeerIndex, data: Bytes) {
        self.controller
            .as_ref()
            .unwrap()
            .0
            .send_message_to(peer, protocol_id, data);
    }

    pub fn receive(&self) -> NetMessage {
        self.controller.as_ref().unwrap().1.recv().unwrap()
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
