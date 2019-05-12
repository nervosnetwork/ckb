use crate::{sleep, Net, Spec};
use log::info;

pub struct Disconnect;

impl Spec for Disconnect {
    fn run(&self, mut net: Net) {
        info!("Running Disconnect");

        info!("Disconnect node1");
        let node1 = net.nodes.pop().unwrap();
        std::mem::drop(node1);
        sleep(10);

        info!("The address of node1 should be removed from node0's peers");
        let peers = net.nodes[0]
            .rpc_client()
            .get_peers()
            .call()
            .expect("rpc call get_peers failed");

        assert!(peers.is_empty());
    }

    fn num_nodes(&self) -> usize {
        2
    }
}
