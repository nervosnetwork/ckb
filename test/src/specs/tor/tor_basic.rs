use crate::{Node, Spec, utils::wait_until};
use ckb_logger::info;

use super::TorServer;

pub struct TorServiceContainsPublicAddr {
    tor_server: TorServer,
}

impl Default for TorServiceContainsPublicAddr {
    fn default() -> Self {
        let tor_server = TorServer::new(None);
        Self { tor_server }
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
        self.tor_server.tor_wait_bootstrap_done();

        let node = &nodes[0];

        let rpc_client = node.rpc_client();
        wait_until(30, || {
            let node_info = rpc_client.local_node_info();

            info!(
                "node_onion_p2p_addr: {:?}",
                node_info
                    .addresses
                    .iter()
                    .map(|addrs| addrs.address.clone())
                    .collect::<Vec<_>>()
            );

            let node_onion_addrs: Vec<_> = node_info
                .addresses
                .iter()
                .filter(|addr| {
                    // check contains the onion address
                    addr.address.contains("/onion3")
                })
                .collect();
            !node_onion_addrs.is_empty()
        });
    }
}
