use crate::{Node, Spec};
use ckb_logger::{error, info};

use super::TorServer;

pub struct TorServiceContainsPublicAddr {
    tor_server: TorServer,
    tor_server_process: std::process::Child,
}

impl Drop for TorServiceContainsPublicAddr {
    fn drop(&mut self) {
        match self.tor_server_process.kill() {
            Ok(_) => info!("tor server process killed"),
            Err(e) => error!("tor server process kill failed: {:?}", e),
        }
    }
}

impl Default for TorServiceContainsPublicAddr {
    fn default() -> Self {
        let tor_server = TorServer::new();
        let tor_server_process = tor_server.tor_start();
        Self {
            tor_server,
            tor_server_process,
        }
    }
}
impl Spec for TorServiceContainsPublicAddr {
    crate::setup!(num_nodes: 1);

    fn before_run(&self) -> Vec<Node> {
        std::thread::sleep(std::time::Duration::from_secs(5));

        let mut node0 = Node::new(self.name(), "node0");
        node0.modify_app_config(|config: &mut ckb_app_config::CKBAppConfig| {
            config.network.onion.listen_on_onion = true;
            config.network.onion.onion_server =
                Some(format!("127.0.0.1:{}", self.tor_server.socks_port));
            config.network.onion.tor_controller =
                format!("127.0.0.1:{}", self.tor_server.control_port);
        });

        node0.start();

        vec![node0]
    }

    fn run(&self, nodes: &mut Vec<Node>) {
        // when _tor_server_guard dropped, the tor server will be killed by Drop

        let node = &nodes[0];

        let rpc_client = node.rpc_client();
        let node_info = rpc_client.local_node_info();

        let node_onion_addrs: Vec<_> = node_info
            .addresses
            .iter()
            .filter(|addr| {
                // check contains the onion address
                info!("addr: {:?}", addr.address);
                addr.address.contains("/onion3")
            })
            .collect();
        assert!(
            !node_onion_addrs.is_empty(),
            "node should contains onion address"
        );

        let node_onion_p2p_addr: String = node_onion_addrs.first().unwrap().address.clone();
        info!("node_onion_p2p_addr: {}", node_onion_p2p_addr);
    }
}
