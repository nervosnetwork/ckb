use crate::utils::{sleep, wait_until};
use crate::{Net, Spec};
use ckb_app_config::CKBAppConfig;
use log::info;
use std::collections::HashSet;

pub struct WhitelistOnSessionLimit;

impl Spec for WhitelistOnSessionLimit {
    crate::name!("whitelist_on_session_limit");

    crate::setup!(num_nodes: 5, connect_all: false);

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig)> {
        // disable outbound peer service
        Box::new(|config| {
            config.network.connect_outbound_interval_secs = 0;
            config.network.discovery_local_address = true;
            config.network.max_peers = 2;
            config.network.max_outbound_peers = 1;
        })
    }

    fn run(&self, net: &mut Net) {
        info!("Running whitelist on session limit");

        // with no whitelist
        let node4 = net.nodes.pop().unwrap();
        let node3 = net.nodes.pop().unwrap();
        let node2 = net.nodes.pop().unwrap();
        let node1 = net.nodes.pop().unwrap();
        let mut node0 = net.nodes.pop().unwrap();

        let mut id_set = HashSet::new();
        id_set.insert(node1.node_id());
        id_set.insert(node4.node_id());

        node0.connect(&node1);
        // outbound session will be refused
        node0.connect_uncheck(&node2);
        node0.generate_blocks(1);
        node3.connect(&node0);
        // inbound session will be rotated by network partition
        node4.connect_uncheck(&node0);

        sleep(5);

        let rpc_client0 = node0.rpc_client();
        let is_connect_peer_num_eq_2 = wait_until(10, || {
            let peers = rpc_client0.get_peers();
            peers.len() == 2
                && peers
                    .into_iter()
                    .all(|node| id_set.contains(&node.node_id.as_str()))
        });

        if !is_connect_peer_num_eq_2 {
            panic!("refuse to connect fail");
        }

        // restart node0, set node1 to node0's whitelist
        let node1_listen = format!(
            "/ip4/127.0.0.1/tcp/{}/p2p/{}",
            node1.p2p_port(),
            node1.node_id()
        );

        node0.stop();

        node0.edit_config_file(
            Box::new(|_| ()),
            Box::new(move |config| {
                config.network.whitelist_peers = vec![node1_listen.parse().unwrap()]
            }),
        );
        node0.start();

        // with whitelist
        let mut id_set = HashSet::new();
        id_set.insert(node1.node_id());
        id_set.insert(node2.node_id());
        id_set.insert(node3.node_id());

        node0.connect(&node2);
        node3.connect(&node0);
        // whitelist will be connected on outbound reach limit
        node0.connect(&node1);

        let rpc_client0 = node0.rpc_client();
        let is_connect_peer_num_eq_3 = wait_until(10, || {
            let peers = rpc_client0.get_peers();
            peers.len() == 3
                && peers
                    .into_iter()
                    .all(|node| id_set.contains(&node.node_id.as_str()))
        });

        if !is_connect_peer_num_eq_3 {
            panic!("whitelist connect fail");
        }
    }
}
