use crate::{sleep, Net, Spec, TestProtocol};
use ckb_protocol::{get_root, SyncMessage, SyncPayload};
use ckb_sync::NetworkProtocol;
use log::info;

pub struct MalformedMessage;

impl Spec for MalformedMessage {
    fn run(&self, net: Net) {
        info!("Running MalformedMessage");

        info!("Connect node0");
        let node0 = &net.nodes[0];
        net.connect(node0);

        info!("Test node should receive GetHeaders message from node0");
        let (peer_id, data) = net.receive();
        let msg = get_root::<SyncMessage>(&data).expect("parse message failed");
        assert_eq!(SyncPayload::GetHeaders, msg.payload_type());

        info!("Send malformed message to node0 twice");
        net.send(NetworkProtocol::SYNC.into(), peer_id, vec![0, 0, 0, 0]);
        sleep(3);
        net.send(NetworkProtocol::SYNC.into(), peer_id, vec![0, 1, 2, 3]);
        sleep(3);

        info!("Node0 should disconnect test node");
        let peers = net.nodes[0]
            .rpc_client()
            .get_peers()
            .call()
            .expect("rpc call get_peers failed");

        assert!(peers.is_empty());

        info!("Node0 should ban test node");
        net.connect(node0);
        sleep(3);
        let peers = net.nodes[0]
            .rpc_client()
            .get_peers()
            .call()
            .expect("rpc call get_peers failed");

        assert!(peers.is_empty());
    }

    fn num_nodes(&self) -> usize {
        1
    }

    fn test_protocols(&self) -> Vec<TestProtocol> {
        vec![TestProtocol {
            id: NetworkProtocol::SYNC.into(),
            protocol_name: "syn".to_string(),
            supported_versions: vec!["1".to_string()],
        }]
    }
}
