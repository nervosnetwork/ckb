use crate::{Node, Spec, utils::wait_until};
use ckb_logger::info;
use ckb_util::Mutex;

use super::TorServer;

pub struct TorConnect {
    tor_server: Mutex<TorServer>,
}

impl Default for TorConnect {
    fn default() -> Self {
        let tor_server = Mutex::new(TorServer::new(None));
        TorConnect { tor_server }
    }
}

impl Spec for TorConnect {
    crate::setup!(num_nodes: 3);

    fn before_run(&self) -> Vec<Node> {
        let tor_controller_url = format!("127.0.0.1:{}", self.tor_server.lock().control_port);
        let mut nodes = (0..self.setup().num_nodes)
            .map(|i| Node::new(self.name(), &format!("node{i}")))
            .collect::<Vec<_>>();
        nodes.iter_mut().for_each(|node| {
            node.modify_app_config(|config: &mut ckb_app_config::CKBAppConfig| {
                config.logger.filter = Some("ckb-network=trace,info".to_string());

                config.network.connect_outbound_interval_secs = 15;

                config.network.onion.listen_on_onion = true;

                config.network.onion.onion_server =
                    Some(format!("127.0.0.1:{}", self.tor_server.lock().socks_port));

                config.network.onion.tor_controller = tor_controller_url.clone();
            });

            node.start();
        });
        nodes
    }

    fn run(&self, nodes: &mut Vec<crate::Node>) {
        self.tor_server.lock().tor_wait_bootstrap_done();

        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        node0.mine_until_out_bootstrap_period();
        info!("node0 tip: {}", node0.get_tip_block_number());
        nodes.iter().for_each(|node| node.mine_until_out_ibd_mode());
        node0.mine(1);
        let (_stop_tx, stop_rx) = ckb_channel::unbounded::<()>();
        std::thread::spawn({
            let node0 = node0.clone();
            move || {
                let tick = ckb_channel::tick(std::time::Duration::from_secs(6));
                loop {
                    ckb_channel::select! {
                        recv(tick) -> _ => {
                            node0.mine(1);
                            let tip = node0.get_tip_block();
                            info!("node0 mined a block, node0's tip: {}-{}", tip.number(), tip.hash());
                        }
                        recv(stop_rx) -> _ => {
                            info!("node0 stopped mine");
                            return;
                        }
                    };
                }
            }
        });

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
            "node0 {} and node1 {} connected, node0 {} and node2 {} connected",
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
        );

        let ticker = ckb_channel::tick(std::time::Duration::from_secs(1));
        ckb_channel::select! {
            recv(ticker) -> _ => {
                [node0, node1, node2].iter().enumerate().for_each(|(i, _node)|
                    {
                let tip = node1.rpc_client().get_tip_block_number();
                    info!("node{} tip: {}", i,tip);
                    }
                )
            }
        };
    }
}
