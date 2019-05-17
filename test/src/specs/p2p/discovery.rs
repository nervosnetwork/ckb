use crate::utils::wait_until;
use crate::{Net, Spec};
use log::info;

pub struct Discovery;

impl Spec for Discovery {
    fn run(&self, net: Net) {
        info!("Running Discovery");
        let node0_id = &net.nodes[0].node_id.clone().unwrap();
        let node2 = &net.nodes[2];
        let mut rpc_client = node2.rpc_client();

        info!("Waiting for discovering");
        let ret = wait_until(10, || {
            rpc_client
                .get_peers()
                .iter()
                .any(|peer| &peer.node_id == node0_id)
        });
        assert!(
            ret,
            "the address of node0 should be discovered by node2 and connected"
        );
    }
}
