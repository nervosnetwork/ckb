use crate::utils::wait_until;
use crate::{Net, Spec};
use log::info;

pub struct Disconnect;

impl Spec for Disconnect {
    fn run(&self, mut net: Net) {
        info!("Running Disconnect");

        info!("Disconnect node1");
        let node1 = net.nodes.pop().unwrap();
        std::mem::drop(node1);

        let rpc_client = net.nodes[0].rpc_client();
        let ret = wait_until(10, || {
            let peers = rpc_client.get_peers();
            peers.is_empty()
        });
        assert!(
            ret,
            "The address of node1 should be removed from node0's peers",
        )
    }

    fn num_nodes(&self) -> usize {
        2
    }
}
