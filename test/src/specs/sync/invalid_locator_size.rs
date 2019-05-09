use crate::{sleep, Net, Spec, TestProtocol};
use ckb_protocol::SyncMessage;
use ckb_sync::{NetworkProtocol, MAX_LOCATOR_SIZE};
use flatbuffers::FlatBufferBuilder;
use log::info;
use numext_fixed_hash::{h256, H256};

pub struct InvalidLocatorSize;

impl Spec for InvalidLocatorSize {
    fn run(&self, net: Net) {
        info!("Running InvalidLocatorSize");

        info!("Connect node0");
        let node0 = &net.nodes[0];
        net.connect(node0);
        // get peer_id from GetHeaders message
        let (peer_id, _, _) = net.receive();
        // generate 1 block on node0, to exit IBD mode.
        node0.generate_block();

        let hashes: Vec<_> = (0..=MAX_LOCATOR_SIZE).map(|_| h256!("0x1")).collect();
        let fbb = &mut FlatBufferBuilder::new();
        let message = SyncMessage::build_get_headers(fbb, &hashes);
        fbb.finish(message, None);
        net.send(
            NetworkProtocol::SYNC.into(),
            peer_id,
            fbb.finished_data().into(),
        );

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
        vec![TestProtocol::sync()]
    }
}
