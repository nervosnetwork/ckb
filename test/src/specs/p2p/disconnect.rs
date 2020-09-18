use crate::utils::wait_until;
use crate::{Node, Spec};
use log::info;

pub struct Disconnect;

impl Spec for Disconnect {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        info!("Running Disconnect");

        info!("Disconnect node1");
        let node1 = nodes.pop().unwrap();
        std::mem::drop(node1);

        let rpc_client = nodes[0].rpc_client();
        let ret = wait_until(10, || {
            let peers = rpc_client.get_peers();
            peers.is_empty()
        });
        assert!(
            ret,
            "The address of node1 should be removed from node0's peers",
        )
    }
}
