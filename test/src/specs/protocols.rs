use crate::{sleep, Net, Spec, TestProtocol};
use ckb_network::ProtocolId;
use ckb_protocol::{SyncMessage, SyncPayload};
use ckb_sync::NetworkProtocol;
use flatbuffers::get_root;
use log::info;

pub struct MalformedMessage;

impl Spec for MalformedMessage {
    fn run(&self, net: Net) {
        info!("Running MalformedMessage");

        info!("Connect node0");
        let node0 = &net.nodes[0];
        net.connect(node0);

        info!("Test node should receive GetHeaders message from node0");
        let (_peer_id, data) = net.receive();
        let msg = get_root::<SyncMessage>(&data);
        assert_eq!(SyncPayload::GetHeaders, msg.payload_type());

        // TODO waiting for https://github.com/nervosnetwork/ckb/pull/364
        // Now, it will print out the error backtrace of node0
        info!("Send malformed message to node0");
        net.send(100, 0, vec![0, 1, 2, 3]);
        sleep(3);

        info!("Node0 should disconnect and ban test node");
        let _peers = net.nodes[0]
            .rpc_client()
            .get_peers()
            .call()
            .expect("rpc call get_peers failed");

        // assert!(peers.is_empty());
    }

    fn num_nodes(&self) -> usize {
        1
    }

    fn test_protocols(&self) -> Vec<TestProtocol> {
        vec![TestProtocol {
            id: NetworkProtocol::SYNC as ProtocolId,
            protocol_name: "syn".to_string(),
            supported_versions: vec![1],
        }]
    }
}
