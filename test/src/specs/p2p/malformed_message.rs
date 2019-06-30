use crate::utils::wait_until;
use crate::{Net, Spec, TestProtocol};
use ckb_protocol::{get_root, SyncMessage, SyncPayload};
use ckb_sync::NetworkProtocol;
use log::info;

pub struct MalformedMessage;

impl Spec for MalformedMessage {
    fn run(&self, net: Net) {
        info!("Connect node0");
        let node0 = &net.nodes[0];
        net.connect(node0);

        info!("Test node should receive GetHeaders message from node0");
        let (peer_id, _, data) = net.receive();
        let msg = get_root::<SyncMessage>(&data).expect("parse message failed");
        assert_eq!(SyncPayload::GetHeaders, msg.payload_type());

        info!("Send malformed message to node0 twice");
        net.send(
            NetworkProtocol::SYNC.into(),
            peer_id,
            vec![0, 0, 0, 0].into(),
        );
        net.send(
            NetworkProtocol::SYNC.into(),
            peer_id,
            vec![0, 1, 2, 3].into(),
        );
        let rpc_client = net.nodes[0].rpc_client();
        let ret = wait_until(10, || rpc_client.get_peers().is_empty());
        assert!(ret, "Node0 should disconnect test node");

        net.connect(node0);
        let ret = wait_until(10, || !rpc_client.get_peers().is_empty());
        assert!(!ret, "Node0 should ban test node");
    }

    fn test_protocols(&self) -> Vec<TestProtocol> {
        vec![TestProtocol::sync()]
    }
}
