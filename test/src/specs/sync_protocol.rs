use crate::{sleep, Net, TestNode, Spec};
use log::info;

pub struct MalformedMessage;

impl Spec for MalformedMessage {
    fn run(&self, net: Net) {
        info!("Running MalformedMessage");
        let node0 = &net.nodes[0];

        info!("Start test node");
        let test_node = TestNode::new();
        info!("Connect test node to node0");
        test_node.connect(node0);
        sleep(10);

        let peers = node0
            .rpc_client()
            .get_peers()
            .call()
            .expect("rpc call get_peers failed");
        info!("Peers {:?}", peers);

        info!("Send malformed message to node0");
        test_node.send(100, 0, vec![0]);

        info!("Node0 should disconnect and ban test node");
    }

    fn num_nodes(&self) -> usize {
        1
    }
}
