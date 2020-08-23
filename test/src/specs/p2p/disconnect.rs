use crate::utils::wait_until;
use crate::{Net, Spec};
use log::info;

pub struct Disconnect;

impl Spec for Disconnect {
    crate::name!("disconnect");

    crate::setup!(num_nodes: 2);

    fn run(&self, net: &mut Net) {
        info!("Running Disconnect");

        info!("Disconnect node1");
        let node1 = net.node(1);
        node1.stop();

        let rpc_client = net.node(0).rpc_client();
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
