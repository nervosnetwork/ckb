use crate::specs::TestProtocol;
use crate::utils::{temp_path, wait_until};
use crate::{Node, Setup};
use ckb_app_config::NetworkConfig;
use ckb_network::{
    bytes::Bytes, CKBProtocol, CKBProtocolContext, CKBProtocolHandler, DefaultExitHandler,
    NetworkController, NetworkService, NetworkState, PeerIndex, ProtocolId,
};
use ckb_types::core::{BlockNumber, BlockView};
use crossbeam_channel::{self, Receiver, RecvTimeoutError, Sender};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU16, Ordering},
    Arc,
};
use std::time::Duration;

pub type NetMessage = (PeerIndex, ProtocolId, Bytes);

pub struct Net {
    pub nodes: Vec<Node>,
    controller: Option<(NetworkController, Receiver<NetMessage>)>,
    p2p_port: u16,
    setup: Setup,
    working_dir: String,
    vendor_dir: PathBuf,
}

impl Net {
    pub fn new(
        binary: &str,
        start_port: Arc<AtomicU16>,
        vendor_dir: PathBuf,
        setup: Setup,
        case_name: &str,
    ) -> Self {
        let p2p_port = start_port.fetch_add(1, Ordering::SeqCst);
        let nodes: Vec<Node> = (0..setup.num_nodes)
            .enumerate()
            .map(|(index, _)| {
                let node_index = "node".to_owned() + &index.to_string();
                let p2p_port = start_port.fetch_add(1, Ordering::SeqCst);
                let rpc_port = start_port.fetch_add(1, Ordering::SeqCst);
                Node::new(binary, p2p_port, rpc_port, case_name, &node_index)
            })
            .collect();

        Self {
            nodes,
            controller: None,
            p2p_port,
            setup,
            working_dir: temp_path(case_name, "net"),
            vendor_dir,
        }
    }

    pub fn working_dir(&self) -> &str {
        &self.working_dir
    }

    pub fn vendor_dir(&self) -> &PathBuf {
        &self.vendor_dir
    }

    fn num_nodes(&self) -> u32 {
        self.setup.num_nodes as u32
    }

    pub fn node_id(&self) -> String {
        self.controller
            .as_ref()
            .map(|(control, _)| control.node_id())
            .expect("uninitialized controller")
    }

    pub fn p2p_port(&self) -> u16 {
        self.p2p_port
    }

    fn test_protocols(&self) -> &[TestProtocol] {
        &self.setup.protocols
    }

    pub fn controller(&self) -> &(NetworkController, Receiver<NetMessage>) {
        self.controller.as_ref().expect("uninitialized controller")
    }

    pub fn init_controller(&self, node: &Node) {
        assert!(
            !self.test_protocols().is_empty(),
            "Net cannot connect the node with empty setup::test_protocols"
        );
        assert!(self.controller.is_none());

        let (tx, rx) = crossbeam_channel::unbounded();
        let config = NetworkConfig {
            listen_addresses: vec![format!("/ip4/127.0.0.1/tcp/{}", self.p2p_port())
                .parse()
                .expect("invalid address")],
            public_addresses: vec![],
            bootnodes: vec![],
            dns_seeds: vec![],
            whitelist_peers: vec![],
            whitelist_only: false,
            max_peers: self.num_nodes(),
            max_outbound_peers: self.num_nodes(),
            path: self.working_dir().into(),
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
            .test_protocols()
            .iter()
            .cloned()
            .map(|tp| {
                CKBProtocol::new(
                    tp.protocol_name,
                    tp.id,
                    &tp.supported_versions,
                    1024 * 1024,
                    Box::new(DummyProtocolHandler { tx: tx.clone() }),
                    Arc::clone(&network_state),
                    Default::default(),
                )
            })
            .collect();

        let controller = Some((
            NetworkService::new(
                Arc::clone(&network_state),
                protocols,
                Vec::new(),
                node.consensus().identify_name(),
                "0.1.0".to_string(),
                DefaultExitHandler::default(),
            )
            .start(Some("NetworkService"))
            .expect("Start network service failed"),
            rx,
        ));

        let ptr = self as *const Self as *mut Self;
        unsafe {
            let _ingore_prev_value = ::std::mem::replace(&mut (*ptr).controller, controller);
        }
    }

    pub fn connect(&self, node: &Node) {
        if self.controller.is_none() {
            self.init_controller(node);
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

    // generate a same block on all nodes, exit IBD mode and return the tip block
    pub fn exit_ibd_mode(&self) -> BlockView {
        let block = self.nodes[0].new_block(None, None, None);
        self.nodes.iter().for_each(|node| {
            node.submit_block(&block);
        });
        block
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

    pub fn should_receive<F>(&self, f: F, message: &str) -> PeerIndex
    where
        F: Fn(&Bytes) -> bool,
    {
        let mut peer_id: PeerIndex = Default::default();
        let received = wait_until(30, || {
            let (receive_peer_id, _, data) = self
                .receive_timeout(Duration::from_secs(30))
                .expect("receive msg");
            peer_id = receive_peer_id;
            f(&data)
        });

        assert!(received, message.to_string());
        peer_id
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
