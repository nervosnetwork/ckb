use crate::utils::wait_until;
use crate::{Node, Spec};

pub struct Discovery;

impl Spec for Discovery {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        for i in 0..nodes.len() - 1 {
            nodes[i].connect(&nodes[i + 1]);
        }

        let all_connected = wait_until(10, || {
            nodes
                .iter()
                .all(|node| node.rpc_client().get_peers().len() == nodes.len() - 1)
        });
        assert!(
            all_connected,
            "nodes should discover and connect each other",
        );
    }

    fn modify_app_config(&self, config: &mut ckb_app_config::CKBAppConfig) {
        // enable outbound peer service to connect discovered peers
        config.network.connect_outbound_interval_secs = 1;
        config.network.discovery_local_address = true;
    }
}
