use crate::node::exit_ibd_mode;
use crate::utils::wait_until;
use crate::{Net, Node, Spec};
use ckb_network::{bytes::Bytes, SupportProtocols};
use ckb_types::{
    packed::{GetHeaders, SyncMessage},
    prelude::*,
};
use log::info;

pub struct MalformedMessage;

impl Spec for MalformedMessage {
    fn run(&self, nodes: &mut Vec<Node>) {
        info!("Run malformed message");
        info!("Connect node0");
        let node0 = &nodes[0];
        exit_ibd_mode(nodes);
        let mut net = Net::new(self.name(), node0.consensus(), vec![SupportProtocols::Sync]);
        net.connect(node0);

        info!("Test node should receive GetHeaders message from node0");
        let ret = net.should_receive(node0, |data: &Bytes| {
            SyncMessage::from_slice(&data)
                .map(|message| message.to_enum().item_name() == GetHeaders::NAME)
                .unwrap_or(false)
        });
        assert!(
            ret,
            "Test node should receive GetHeaders message from node0"
        );

        info!("Send malformed message to node0 twice");
        net.send(node0, SupportProtocols::Sync, vec![0, 0, 0, 0].into());
        net.send(node0, SupportProtocols::Sync, vec![0, 1, 2, 3].into());
        let rpc_client = nodes[0].rpc_client();
        let ret = wait_until(10, || rpc_client.get_peers().is_empty());
        assert!(ret, "Node0 should disconnect test node");
        let ret = wait_until(10, || {
            rpc_client
                .get_banned_addresses()
                .iter()
                .any(|ban| ban.address == "127.0.0.1/32")
        });
        assert!(ret, "Node0 should ban test node");
    }
}

pub struct MalformedMessageWithWhitelist;

impl Spec for MalformedMessageWithWhitelist {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        info!("Run malformed message with whitelist");
        let node1 = nodes.pop().unwrap();
        exit_ibd_mode(nodes);
        let mut node0 = nodes.pop().unwrap();
        let mut net = Net::new(self.name(), node0.consensus(), vec![SupportProtocols::Sync]);
        net.connect(&node0);

        info!("Test node should receive GetHeaders message from node0");
        let ret = net.should_receive(&node0, |data: &Bytes| {
            SyncMessage::from_slice(&data)
                .map(|message| message.to_enum().item_name() == GetHeaders::NAME)
                .unwrap_or(false)
        });
        assert!(
            ret,
            "Test node should receive GetHeaders message from node0"
        );

        node0.stop();
        node0.modify_app_config(|config| {
            config.network.whitelist_peers = vec![net.p2p_address().parse().unwrap()]
        });
        node0.start();
        net.connect(&node0);

        let rpc_client = node0.rpc_client();
        let ret = wait_until(10, || rpc_client.get_peers().len() == 1);
        assert!(ret, "Node0 should connect test node");

        info!("Send malformed message to node0 twice");
        net.send(&node0, SupportProtocols::Sync, vec![0, 0, 0, 0].into());
        net.send(&node0, SupportProtocols::Sync, vec![0, 1, 2, 3].into());

        node1.connect(&node0);

        let rpc_client = node0.rpc_client();
        let ret = wait_until(10, || rpc_client.get_peers().len() == 2);
        assert!(ret, "Node0 should keep connection with test node");
        let ret = wait_until(10, || rpc_client.get_banned_addresses().is_empty());
        assert!(ret, "Node0 should not ban test node");
    }
}
