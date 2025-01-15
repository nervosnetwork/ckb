use ckb_logger::{error, info};

use crate::{utils::wait_until, Node, Spec};

use super::TorServer;

pub struct TorConnect {
    tor_server: TorServer,
    tor_server_process: std::process::Child,
}

impl Default for TorConnect {
    fn default() -> Self {
        let tor_server = TorServer::new();
        let tor_server_process = tor_server.tor_start();
        TorConnect {
            tor_server,
            tor_server_process,
        }
    }
}

impl Drop for TorConnect {
    fn drop(&mut self) {
        match self.tor_server_process.kill() {
            Ok(_) => info!("tor server process killed"),
            Err(e) => error!("tor server process kill failed: {:?}", e),
        }
    }
}

impl Spec for TorConnect {
    crate::setup!(num_nodes: 3);

    fn before_run(&self) -> Vec<Node> {
        let mut nodes = (0..self.setup().num_nodes)
            .map(|i| Node::new(self.name(), &format!("node{i}")))
            .collect::<Vec<_>>();
        nodes.iter_mut().for_each(|node| {
            node.modify_app_config(|config: &mut ckb_app_config::CKBAppConfig| {
                config.logger.filter = Some("ckb-network=trace,info".to_string());

                config.network.connect_outbound_interval_secs = 15;

                config.network.onion.listen_on_onion = true;

                // config.network.onion.onion_server = Some(format!("socks5://127.0.0.1:9050"));
                // config.network.onion.tor_controller = format!("127.0.0.1:9051");

                config.network.onion.onion_server =
                    Some(format!("socks5://127.0.0.1:{}", self.tor_server.socks_port));

                config.network.onion.tor_controller =
                    format!("127.0.0.1:{}", self.tor_server.control_port);

                let p2p_addr = config.network.listen_addresses.first().unwrap().to_string();

                let p2p_port: u16 = p2p_addr.split("/tcp/").last().unwrap().parse().unwrap();
                info!("node p2p listen port: {}", p2p_port);

                config.network.onion.onion_service_target = Some(format!("127.0.0.1:{}", p2p_port));
            });

            node.start();
        });
        nodes
    }

    fn run(&self, nodes: &mut Vec<crate::Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        node0.mine_until_out_bootstrap_period();
        info!("node0 tip: {}", node0.get_tip_block_number());
        nodes.iter().for_each(|node| node.mine_until_out_ibd_mode());

        info!(
            "node0 {} connecting to node1 {}",
            node0.node_id(),
            node1.node_id()
        );
        node1.connect_onion(node0);
        info!(
            "node0 {} connecting to node2 {}",
            node0.node_id(),
            node2.node_id()
        );
        node2.connect_onion(node0);
        info!(
            "node0 {} and node1 {} connected, node0 {} and node2{} conencted",
            node0.get_onion_public_addr().unwrap(),
            node1.get_onion_public_addr().unwrap(),
            node0.get_onion_public_addr().unwrap(),
            node2.get_onion_public_addr().unwrap(),
        );

        let node1_node2_connected = wait_until(180, || {
            let nodes_peers: Vec<Vec<String>> = nodes
                .iter()
                .map(|node| {
                    let node_peers: Vec<String> = node
                        .rpc_client()
                        .get_peers()
                        .iter()
                        .flat_map(|addr| {
                            addr.addresses
                                .iter()
                                .map(|addr| addr.address.to_owned())
                                .collect::<Vec<String>>()
                        })
                        .collect();
                    node_peers
                })
                .collect();
            let node0_peers = nodes_peers[0].clone();
            let node1_peers = nodes_peers[1].clone();
            let node2_peers = nodes_peers[2].clone();

            info!("node0_peers: {:?}", node0_peers);
            info!("node1_peers: {:?}", node1_peers);
            info!("node2_peers: {:?}", node2_peers);
            node1_peers
                .into_iter()
                .filter(|addr| addr.starts_with("/onion3/"))
                .collect::<Vec<_>>()
                .contains(&node2.get_onion_public_addr().unwrap())
                && node2_peers
                    .into_iter()
                    .filter(|addr| addr.starts_with("/onion3/"))
                    .collect::<Vec<_>>()
                    .contains(&node1.get_onion_public_addr().unwrap())
        });
        assert!(
            node1_node2_connected,
            "node1 {} and node2 {} not connected",
            node1.get_onion_public_addr().unwrap(),
            node2.get_onion_public_addr().unwrap(),
        );
        info!(
            "node1 {} and node2 {} are connected",
            node1.get_onion_public_addr().unwrap(),
            node2.get_onion_public_addr().unwrap(),
        )
    }
}
