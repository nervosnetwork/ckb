use crate::global::VENDOR_PATH;
use crate::utils::{find_available_port, temp_path, wait_until};
use crate::Node;
use ckb_app_config::NetworkConfig;
use ckb_chain_spec::consensus::Consensus;

use ckb_channel::{self as channel, Receiver, RecvTimeoutError, Sender};
use ckb_network::{
    bytes::Bytes, CKBProtocol, CKBProtocolContext, CKBProtocolHandler, DefaultExitHandler,
    NetworkController, NetworkService, NetworkState, PeerIndex, ProtocolId, SupportProtocols,
};

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub type NetMessage = (PeerIndex, ProtocolId, Bytes);

pub struct Net {
    consensus: Consensus,
    protocols: Vec<SupportProtocols>,
    p2p_port: u16,
    working_dir: String,
    controller: Option<(NetworkController, Receiver<NetMessage>)>,
}

impl Net {
    pub fn new(spec_name: &str, consensus: Consensus, protocols: Vec<SupportProtocols>) -> Self {
        assert!(
            !protocols.is_empty(),
            "Net cannot initialize with empty protocols"
        );
        let p2p_port = find_available_port();
        let working_dir = temp_path(spec_name, "net");
        Self {
            consensus,
            protocols,
            p2p_port,
            working_dir,
            controller: None,
        }
    }

    pub fn working_dir(&self) -> &str {
        &self.working_dir
    }

    pub fn vendor_dir(&self) -> PathBuf {
        let vendor_path = VENDOR_PATH.lock();
        (*vendor_path).clone()
    }

    pub fn p2p_listen(&self) -> String {
        format!("/ip4/127.0.0.1/tcp/{}", self.p2p_port)
    }

    pub fn p2p_address(&self) -> String {
        format!(
            "/ip4/127.0.0.1/tcp/{}/p2p/{}",
            self.p2p_port,
            self.node_id()
        )
    }

    pub fn node_id(&self) -> String {
        if self.controller.is_none() {
            self.init_controller()
        }
        self.controller().0.node_id()
    }

    pub fn controller(&self) -> &(NetworkController, Receiver<NetMessage>) {
        self.controller.as_ref().expect("uninitialized controller")
    }

    fn init_controller(&self) {
        assert!(self.controller.is_none());

        let (tx, rx) = channel::unbounded();
        let config = NetworkConfig {
            listen_addresses: vec![self.p2p_listen().parse().expect("invalid address")],
            public_addresses: vec![],
            bootnodes: vec![],
            dns_seeds: vec![],
            whitelist_peers: vec![],
            whitelist_only: false,
            max_peers: 128,
            max_outbound_peers: 128,
            path: self.working_dir().into(),
            ping_interval_secs: 15,
            ping_timeout_secs: 20,
            connect_outbound_interval_secs: 0,
            discovery_local_address: true,
            upnp: false,
            bootnode_mode: false,
            max_send_buffer: None,
            sync: None,
        };

        let network_state =
            Arc::new(NetworkState::from_config(config).expect("Init network state failed"));
        let protocols = self
            .protocols
            .iter()
            .map(|tp| {
                CKBProtocol::new_with_support_protocol(
                    tp.clone(),
                    Box::new(DummyProtocolHandler { tx: tx.clone() }),
                    Arc::clone(&network_state),
                )
            })
            .collect();
        let controller = Some((
            NetworkService::new(
                Arc::clone(&network_state),
                protocols,
                Vec::new(),
                self.consensus.identify_name(),
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
            self.init_controller();
        }
        self.controller().0.add_node(
            &node.node_id().parse().unwrap(),
            node.p2p_address().parse().unwrap(),
        );
    }

    pub fn send(&self, protocol_id: ProtocolId, peer: PeerIndex, data: Bytes) {
        self.controller()
            .0
            .send_message_to(peer, protocol_id, data)
            .expect("Send message to p2p network failed");
    }

    pub fn receive(&self) -> NetMessage {
        self.controller().1.recv().unwrap()
    }

    pub fn receive_timeout(&self, timeout: Duration) -> Result<NetMessage, RecvTimeoutError> {
        self.controller().1.recv_timeout(timeout)
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
