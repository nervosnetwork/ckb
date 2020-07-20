use crate::utils::wait_until;
use crate::{Net, Spec};
use ckb_app_config::CKBAppConfig;
use log::info;

pub struct Discovery;

impl Spec for Discovery {
    crate::name!("discovery");

    crate::setup!(num_nodes: 3);

    fn run(&self, net: &mut Net) {
        let node0_id = net.nodes[0].node_id();
        let node2 = &net.nodes[2];
        let rpc_client = node2.rpc_client();

        info!("Waiting for discovering");
        let ret = wait_until(10, || {
            rpc_client
                .get_peers()
                .iter()
                .any(|peer| peer.node_id == node0_id)
        });
        assert!(
            ret,
            "the address of node0 should be discovered by node2 and connected"
        );
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig)> {
        // enable outbound peer service to connect discovered peers
        Box::new(|config| {
            config.network.connect_outbound_interval_secs = 1;
            config.network.discovery_local_address = true;
        })
    }
}
