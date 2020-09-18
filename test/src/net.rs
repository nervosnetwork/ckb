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
    p2p_port: u16,
    working_dir: PathBuf,
    controller: NetworkController,
    receiver: Receiver<NetMessage>,
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
                ..Default::default()
            })
            .unwrap(),
        );
        let (sender, receiver) = channel::unbounded();
        let protocols = protocols
            .into_iter()
            .map(|protocol| {
                CKBProtocol::new_with_support_protocol(
                    protocol,
                    Box::new(DummyProtocolHandler { tx: sender.clone() }),
                    Arc::clone(&network_state),
                )
            })
            .collect();
        let controller = NetworkService::new(
            Arc::clone(&network_state),
            protocols,
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
            controller,
            receiver,
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

    pub fn connect(&self, node: &Node) {
        self.controller().add_node(
            &node.node_id().parse().unwrap(),
            node.p2p_address().parse().unwrap(),
        );
    }

    pub fn send(&self, protocol_id: ProtocolId, peer: PeerIndex, data: Bytes) {
        self.controller()
            .send_message_to(peer, protocol_id, data)
            .expect("Send message to p2p network failed");
    }

    pub fn receive(&self) -> NetMessage {
        self.receiver.recv().unwrap()
    }

    pub fn receive_timeout(&self, timeout: Duration) -> Result<NetMessage, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
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
