use crate::util::mining::out_ibd_mode;
use crate::utils::wait_until;
use crate::{Node, Spec};
use std::thread::sleep;
use std::time::Duration;

pub struct Discovery;

impl Spec for Discovery {
    crate::setup!(num_nodes: 3);

    fn before_run(&self) -> Vec<Node> {
        let mut nodes = (0..self.setup().num_nodes)
            .map(|i| {
                Node::new_with_modifiers(
                    self.name(),
                    &format!("node{i}"),
                    |config| self.modify_app_config(config),
                    |spec| self.modify_chain_spec(spec),
                )
            })
            .collect::<Vec<_>>();
        // Stagger startup slightly so OutboundPeerService 1s timers on node0 and node2
        // do not phase-lock into simultaneous feeler/outbound dial collisions.
        crate::utils::run_in_spec_scope(nodes.iter_mut().enumerate(), |(i, node)| {
            if i > 0 {
                sleep(Duration::from_millis(250 * i as u64));
            }
            node.start();
        });
        nodes
    }

    fn run(&self, nodes: &mut Vec<Node>) {
        out_ibd_mode(nodes);
        for i in 0..nodes.len() - 1 {
            nodes[i].connect(&nodes[i + 1]);
        }

        let all_connected = wait_until(45, || {
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
        config.network.discovery_announce_check_interval_secs = Some(1);
    }
}
