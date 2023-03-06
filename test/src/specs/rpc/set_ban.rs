use crate::node::{connect_all, waiting_for_sync};
use crate::specs::spec_name;
use crate::util::mining::out_ibd_mode;
use crate::utils::find_available_port;
use crate::utils::wait_until;
use crate::{Node, Spec};

pub struct RpcSetBan;

const BAD_NODE_IP_1: &str = "127.0.0.2";
const BAD_NODE_IP_2: &str = "127.0.1.1";
const BAD_NODE_NETWORK: &str = "127.0.0.0/16";

impl Spec for RpcSetBan {
    // node will ban the node with ip_address and ban the node within the ip/subnet
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];

        out_ibd_mode(nodes);
        connect_all(nodes);

        waiting_for_sync(nodes);

        assert_eq!(node.rpc_client().get_peers().len(), 2);

        node.rpc_client().set_ban(
            BAD_NODE_IP_1.to_owned(),
            "insert".to_owned(),
            None,
            None,
            Some("for_test".to_owned()),
        );
        let ret = wait_until(10, || {
            let peers_cnt = node.rpc_client().get_peers().len();
            peers_cnt == 1
        });
        assert!(ret, "Node1 should ban banned_ip_node");

        node.rpc_client().set_ban(
            BAD_NODE_NETWORK.to_owned(),
            "insert".to_owned(),
            None,
            None,
            Some("for_test".to_owned()),
        );
        let ret = wait_until(10, || {
            let peers_cnt = node.rpc_client().get_peers().len();
            peers_cnt == 0
        });
        assert!(ret, "Node1 should ban banned_ipsubnet_node");
    }

    fn before_run(&self) -> Vec<Node> {
        let mut node = Node::new(spec_name(self), "node");
        node.start();

        let mut banned_ip_node = Node::new(spec_name(self), "banned_ip_node");
        banned_ip_node.modify_app_config(|app_config| {
            let p2p_port = find_available_port();
            app_config.network.listen_addresses =
                vec![format!("/ip4/{BAD_NODE_IP_1}/tcp/{p2p_port}")
                    .parse()
                    .unwrap()];
        });
        banned_ip_node.start();

        let mut banned_ipsubnet_node = Node::new(spec_name(self), "banned_ipsubnet_node");
        banned_ipsubnet_node.modify_app_config(|app_config| {
            let p2p_port = find_available_port();
            app_config.network.listen_addresses =
                vec![format!("/ip4/{BAD_NODE_IP_2}/tcp/{p2p_port}")
                    .parse()
                    .unwrap()];
        });
        banned_ipsubnet_node.start();

        vec![node, banned_ip_node, banned_ipsubnet_node]
    }
}
