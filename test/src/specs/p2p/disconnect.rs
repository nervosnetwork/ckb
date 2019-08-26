use crate::utils::wait_until;
use crate::{Net, Spec};
use log::info;

pub struct Disconnect;

impl Spec for Disconnect {
    crate::name!("disconnect");

    crate::setup!(num_nodes: 2);

    fn run(&self, mut net: Net) {
        info!("Running Disconnect");

        info!("Disconnect node1");
        let node1 = net.nodes.pop().unwrap();
        std::mem::drop(node1);

        let node0 = &net.nodes[0];
        let ret = wait_until(10, || {
            let peers = node0.get_peers();
            peers.is_empty()
        });
        assert!(
            ret,
            "The address of node1 should be removed from node0's peers",
        )
    }
}
