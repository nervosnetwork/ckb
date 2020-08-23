use crate::utils::wait_until;
use crate::{Net, Spec, TestProtocol};
use ckb_network::{bytes::Bytes, SupportProtocols};
use ckb_types::{
    packed::{GetHeaders, SyncMessage},
    prelude::*,
};
use log::info;

pub struct MalformedMessage;

impl Spec for MalformedMessage {
    crate::name!("malformed_message");

    crate::setup!(protocols: vec![TestProtocol::sync()]);

    fn run(&self, net: &mut Net) {
        info!("Run malformed message");
        info!("Connect node0");
        let node0 = net.node(0);
        net.exit_ibd_mode();
        net.connect(node0);

        info!("Test node should receive GetHeaders message from node0");
        let peer_id = net.should_receive(
            |data: &Bytes| {
                SyncMessage::from_slice(&data)
                    .map(|message| message.to_enum().item_name() == GetHeaders::NAME)
                    .unwrap_or(false)
            },
            "Test node should receive GetHeaders message from node0",
        );

        info!("Send malformed message to node0 twice");
        net.send(
            SupportProtocols::Sync.protocol_id(),
            peer_id,
            vec![0, 0, 0, 0].into(),
        );
        net.send(
            SupportProtocols::Sync.protocol_id(),
            peer_id,
            vec![0, 1, 2, 3].into(),
        );
        let rpc_client = net.node(0).rpc_client();
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
    crate::name!("malformed_message_with_whitelist");

    crate::setup!(num_nodes: 2, protocols: vec![TestProtocol::sync()]);

    fn run(&self, net: &mut Net) {
        info!("Run malformed message with whitelist");
        let node1 = net.node(1);
        net.exit_ibd_mode();
        let node0 = net.node(0);
        net.connect(&node0);

        info!("Test node should receive GetHeaders message from node0");
        let peer_id = net.should_receive(
            |data: &Bytes| {
                SyncMessage::from_slice(&data)
                    .map(|message| message.to_enum().item_name() == GetHeaders::NAME)
                    .unwrap_or(false)
            },
            "Test node should receive GetHeaders message from node0",
        );

        let net_listen = format!(
            "/ip4/127.0.0.1/tcp/{}/p2p/{}",
            net.p2p_port(),
            net.node_id()
        );

        node0.stop();

        node0.edit_config_file(
            Box::new(|_| ()),
            Box::new(move |config| {
                config.network.whitelist_peers = vec![net_listen.parse().unwrap()]
            }),
        );
        node0.start();

        net.connect(&node0);

        let rpc_client = node0.rpc_client();
        let ret = wait_until(10, || rpc_client.get_peers().len() == 1);
        assert!(ret, "Node0 should connect test node");

        info!("Send malformed message to node0 twice");
        net.send(
            SupportProtocols::Sync.protocol_id(),
            peer_id,
            vec![0, 0, 0, 0].into(),
        );
        net.send(
            SupportProtocols::Sync.protocol_id(),
            peer_id,
            vec![0, 1, 2, 3].into(),
        );

        node1.connect(&node0);

        let rpc_client = node0.rpc_client();
        let ret = wait_until(10, || rpc_client.get_peers().len() == 2);
        assert!(ret, "Node0 should keep connection with test node");
        let ret = wait_until(10, || rpc_client.get_banned_addresses().is_empty());
        assert!(ret, "Node0 should not ban test node");
    }
}
