use crate::{sleep, Net, Spec};
use log::info;

pub struct Discovery;

impl Spec for Discovery {
    fn run(&self, net: Net) {
        info!("Running Discovery");
        let node0_id = &net.nodes[0].node_id.clone().unwrap();
        let node2 = &net.nodes[2];

        info!("Waiting for discovering");
        sleep(10);

        info!("The address of node0 should be discovered by node2 and connected");
        let peers = node2
            .rpc_client()
            .get_peers()
            .call()
            .expect("rpc call get_peers failed");
        assert!(peers.iter().any(|peer| &peer.node_id == node0_id));
    }
}
