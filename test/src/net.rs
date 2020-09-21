use crate::utils::{find_available_port, message_name, temp_path, wait_until};
use crate::Node;
use ckb_app_config::NetworkConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_channel::{self as channel, unbounded, Receiver, RecvTimeoutError, Sender};
use ckb_network::{
    bytes::Bytes, CKBProtocol, CKBProtocolContext, CKBProtocolHandler, DefaultExitHandler,
    NetworkController, NetworkService, NetworkState, PeerIndex, ProtocolId, SupportProtocols,
};
use ckb_util::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub type NetMessage = (PeerIndex, ProtocolId, Bytes);

pub struct Net {
    p2p_port: u16,
    working_dir: PathBuf,
    protocols: Vec<SupportProtocols>,
    controller: NetworkController,
    register_rx: Receiver<(String, PeerIndex, Receiver<NetMessage>)>,
    receivers: HashMap<String, (PeerIndex, Receiver<NetMessage>)>,
}

impl Net {
    pub fn new(spec_name: &str, consensus: &Consensus, protocols: Vec<SupportProtocols>) -> Self {
        assert!(
            !protocols.is_empty(),
            "Net cannot initialize with empty protocols"
        );
        let p2p_port = find_available_port();
        let working_dir = temp_path(spec_name, "net");

        let p2p_listen = format!("/ip4/127.0.0.1/tcp/{}", p2p_port).parse().unwrap();
        let network_state = Arc::new(
            NetworkState::from_config(NetworkConfig {
                listen_addresses: vec![p2p_listen],
                path: (&working_dir).into(),
                max_peers: 128,
                max_outbound_peers: 128,
                discovery_local_address: true,
                ping_interval_secs: 15,
                ping_timeout_secs: 20,
                ..Default::default()
            })
            .unwrap(),
        );
        let (register_tx, register_rx) = channel::unbounded();
        let protocol_handler = DummyProtocolHandler::new(register_tx);
        let ckb_protocols = protocols
            .iter()
            .map(|protocol| {
                CKBProtocol::new_with_support_protocol(
                    protocol.to_owned(),
                    Box::new(protocol_handler.clone()),
                    Arc::clone(&network_state),
                )
            })
            .collect();
        let controller = NetworkService::new(
            Arc::clone(&network_state),
            ckb_protocols,
            Vec::new(),
            consensus.identify_name(),
            "0.1.0".to_string(),
            DefaultExitHandler::default(),
        )
        .start(Some("NetworkService"))
        .unwrap();
        Self {
            p2p_port,
            working_dir,
            protocols,
            controller,
            register_rx,
            receivers: Default::default(),
        }
    }

    pub fn working_dir(&self) -> &PathBuf {
        &self.working_dir
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
        self.controller().node_id()
    }

    pub fn controller(&self) -> &NetworkController {
        &self.controller
    }

    pub fn connect(&mut self, node: &Node) {
        self.controller().add_node(
            &node.node_id().parse().unwrap(),
            node.p2p_address().parse().unwrap(),
        );

        let mut n_protocols_connected = 0;
        while let Ok(connected) = self.register_rx.recv_timeout(Duration::from_secs(60)) {
            let (node_id, peer_index, receiver) = connected;
            n_protocols_connected += 1;
            if n_protocols_connected == 1 {
                self.receivers
                    .insert(node_id.clone(), (peer_index, receiver));
            }
            if node_id == node.node_id() && n_protocols_connected == self.protocols.len() {
                return;
            }
        }
        panic!("timeout to connect to {}", node.p2p_address());
    }

    pub fn connect_uncheck(&self, node: &Node) {
        self.controller().add_node(
            &node.node_id().parse().unwrap(),
            node.p2p_address().parse().unwrap(),
        );
    }

    pub fn send(&self, node: &Node, protocol: SupportProtocols, data: Bytes) {
        let node_id = node.node_id();
        let protocol_id = protocol.protocol_id();
        let peer_index = self
            .receivers
            .get(node_id)
            .map(|(peer_index, _)| *peer_index)
            .unwrap_or_else(|| panic!("not connected peer {}", node.p2p_address()));
        self.controller()
            .send_message_to(peer_index, protocol_id, data)
            .expect("Send message to p2p network failed");
    }

    pub fn receive_timeout(
        &self,
        node: &Node,
        timeout: Duration,
    ) -> Result<NetMessage, RecvTimeoutError> {
        let node_id = node.node_id();
        let (peer_index, receiver) = self
            .receivers
            .get(node_id)
            .unwrap_or_else(|| panic!("not connected peer {}", node.p2p_address()));
        let net_message = receiver.recv_timeout(timeout)?;
        log::info!(
            "received from peer-{}, message_name: {}",
            peer_index,
            message_name(&net_message.2)
        );
        Ok(net_message)
    }

    pub fn should_receive<Predicate>(&self, node: &Node, predicate: Predicate) -> bool
    where
        Predicate: Fn(&Bytes) -> bool,
    {
        let timeout = Duration::from_secs(30);
        wait_until(30, || {
            self.receive_timeout(node, timeout)
                .map(|(_, _, data)| predicate(&data))
                .unwrap_or(false)
        })
    }
}

pub struct DummyProtocolHandler {
    // When a new peer connects, register to notice outside controller.
    register_tx: Sender<(String, PeerIndex, Receiver<NetMessage>)>,

    // #{node_id => receiver}
    // shared between multiple protocol handlers
    senders: Arc<Mutex<HashMap<String, Sender<NetMessage>>>>,
}

impl Clone for DummyProtocolHandler {
    fn clone(&self) -> Self {
        DummyProtocolHandler {
            register_tx: self.register_tx.clone(),
            senders: Arc::clone(&self.senders),
        }
    }
}

impl DummyProtocolHandler {
    fn new(register_tx: Sender<(String, PeerIndex, Receiver<NetMessage>)>) -> Self {
        Self {
            register_tx,
            senders: Arc::new(Mutex::new(Default::default())),
        }
    }
}

impl CKBProtocolHandler for DummyProtocolHandler {
    fn init(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>) {}

    fn connected(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        _version: &str,
    ) {
        let peer = nc.get_peer(peer_index).unwrap();
        let node_id = peer.peer_id.to_base58();
        let (sender, receiver) = unbounded();
        let mut senders = self.senders.lock();
        if !senders.contains_key(&node_id) {
            senders.insert(node_id.clone(), sender);
        }
        let _ = self.register_tx.send((node_id, peer_index, receiver));
    }

    fn disconnected(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>, peer_index: PeerIndex) {
        if let Some(peer) = nc.get_peer(peer_index) {
            let node_id = peer.peer_id.to_base58();
            self.senders.lock().remove(&node_id);
        }
    }

    fn received(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        data: Bytes,
    ) {
        if let Some(peer) = nc.get_peer(peer_index) {
            let node_id = peer.peer_id.to_base58();
            if let Some(sender) = self.senders.lock().get(&node_id) {
                let _ = sender.send((peer_index, nc.protocol_id(), data));
            }
        }
    }
}
