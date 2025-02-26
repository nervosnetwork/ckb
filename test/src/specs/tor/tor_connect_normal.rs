use ckb_logger::{error, info};

use crate::{utils::wait_until, Node, Spec};

use super::TorServer;

pub struct TorConnectNormal {
    tor_server: TorServer,
    tor_server_process: std::process::Child,
}

impl Default for TorConnectNormal {
    fn default() -> Self {
        let tor_server = TorServer::new();
        let tor_server_process = tor_server.tor_start();
        TorConnectNormal {
            tor_server,
            tor_server_process,
        }
    }
}

impl Drop for TorConnectNormal {
    fn drop(&mut self) {
        match self.tor_server_process.kill() {
            Ok(_) => info!("tor server process killed"),
            Err(e) => error!("tor server process kill failed: {:?}", e),
        }
    }
}

impl Spec for TorConnectNormal {
    crate::setup!(num_nodes: 2);

    fn before_run(&self) -> Vec<Node> {
        std::thread::sleep(std::time::Duration::from_secs(60));
        let mut nodes = (0..self.setup().num_nodes)
            .map(|i| Node::new(self.name(), &format!("node{i}")))
            .collect::<Vec<_>>();
        nodes[1].modify_app_config(|config: &mut ckb_app_config::CKBAppConfig| {
            config.logger.filter = Some("ckb-network=trace,info".to_string());

            config.network.connect_outbound_interval_secs = 15;
            config.network.proxy.proxy_url =
                Some(format!("socks5://127.0.0.1:{}", self.tor_server.socks_port));

            config.network.onion.listen_on_onion = true;
            config.network.onion.onion_server =
                Some(format!("socks5://127.0.0.1:{}", self.tor_server.socks_port));

            config.network.onion.tor_controller =
                format!("127.0.0.1:{}", self.tor_server.control_port);

            let p2p_addr = config.network.listen_addresses.first().unwrap().to_string();

            let p2p_port: u16 = p2p_addr.split("/tcp/").last().unwrap().parse().unwrap();
            info!("node p2p listen port: {}", p2p_port);

            config.network.onion.onion_service_target = Some(format!("127.0.0.1:{}", p2p_port));
        });

        nodes[0].start();
        nodes[1].start();
        nodes
    }

    fn run(&self, nodes: &mut Vec<crate::Node>) {
        let node_normal = &nodes[0];
        let node_onion = &nodes[1];

        node_normal.mine_until_out_bootstrap_period();
        info!("node_normal tip: {}", node_normal.get_tip_block_number());

        node_onion.connect(node_normal);
        let node_onion_node_normal_synced = wait_until(20, || {
            info!("node_onion tip: {}", node_onion.get_tip_block_number());
            node_onion
                .get_tip_block_number()
                .eq(&node_normal.get_tip_block_number())
        });
        assert!(
            node_onion_node_normal_synced,
            "node_onion and node_normal are synced"
        );
        info!(
            "node_onion and node_normal are synced: {}, {}",
            node_onion.get_tip_block_number(),
            node_normal.get_tip_block_number()
        );
    }
}
