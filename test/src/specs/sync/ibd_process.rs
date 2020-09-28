use crate::utils::{sleep, wait_until};
use crate::{Node, Spec};
use log::info;

pub struct IBDProcess;

impl Spec for IBDProcess {
    crate::setup!(num_nodes: 7);

    fn run(&self, nodes: &mut Vec<Node>) {
        info!("Running IBD process");

        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];
        let node3 = &nodes[3];
        let node4 = &nodes[4];
        let node5 = &nodes[5];
        let node6 = &nodes[6];

        node0.connect(node1);
        node0.connect(node2);
        node0.connect(node3);
        node0.connect(node4);
        // The node's outbound connection does not retain the peer which in the ibd state
        node0.generate_blocks(1);
        // will never connect
        node0.connect_uncheck(node5);
        node0.connect_uncheck(node6);

        sleep(5);

        let rpc_client0 = node0.rpc_client();
        let is_connect_peer_num_eq_4 = wait_until(10, || {
            let peers = rpc_client0.get_peers();
            peers.len() == 4
        });

        if !is_connect_peer_num_eq_4 {
            panic!("refuse to connect fail");
        }

        // IBD only with outbound/whitelist node
        let rpc_client1 = node1.rpc_client();
        let rpc_client2 = node2.rpc_client();
        let rpc_client3 = node3.rpc_client();
        let rpc_client4 = node4.rpc_client();
        let rpc_client5 = node5.rpc_client();
        let rpc_client6 = node6.rpc_client();

        let is_nodes_ibd_sync = wait_until(10, || {
            let header1 = rpc_client1.get_tip_header();
            let header2 = rpc_client2.get_tip_header();
            let header3 = rpc_client3.get_tip_header();
            let header4 = rpc_client4.get_tip_header();
            let header5 = rpc_client5.get_tip_header();
            let header6 = rpc_client6.get_tip_header();

            header1.inner.number.value() == 0
                && header1 == header6
                && header1 == header5
                && header1 == header4
                && header1 == header3
                && header1 == header2
        });

        assert!(is_nodes_ibd_sync, "node 1-6 must not sync with node0");

        node5.connect(node0);
        node6.connect(node0);

        let is_node_sync = wait_until(10, || {
            let header5 = rpc_client5.get_tip_header();
            let header6 = rpc_client6.get_tip_header();
            header5 == header6 && header5.inner.number.value() == 1
        });

        assert!(is_node_sync, "node 5-6 must sync with node0");
    }
}

pub struct IBDProcessWithWhiteList;

impl Spec for IBDProcessWithWhiteList {
    crate::setup!(num_nodes: 7);

    fn run(&self, nodes: &mut Vec<Node>) {
        info!("Running IBD process with whitelist");

        {
            nodes[0].stop();

            // whitelist will be connected on outbound on node start
            let node6_address = nodes[6].p2p_address();
            nodes[0].modify_app_config(move |config| {
                config.network.whitelist_peers = vec![node6_address.parse().unwrap()];
            });
            nodes[0].start();
        }

        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];
        let node3 = &nodes[3];
        let node4 = &nodes[4];
        let node5 = &nodes[5];
        let node6 = &nodes[6];

        node0.connect(node1);
        node0.connect(node2);
        node0.connect(node3);
        node0.connect(node4);

        // will never connect, protect node default is 4, see
        // https://github.com/nervosnetwork/ckb/blob/da8897dbc8382293bdf8fadea380a0b79c1efa92/sync/src/lib.rs#L57
        node0.connect_uncheck(node5);

        let rpc_client0 = node0.rpc_client();
        let is_connect_peer_num_eq_5 = wait_until(10, || {
            let peers = rpc_client0.get_peers();
            peers.len() == 5
        });

        if !is_connect_peer_num_eq_5 {
            panic!("refuse to connect fail");
        }

        // After the whitelist is disconnected, it will always try to reconnect.
        // In order to ensure that the node6 has already generated two blocks when reconnecting,
        // it must be in the connected state, and then disconnected.
        node6.generate_blocks(2);

        let generate_res = wait_until(10, || nodes[6].get_tip_block_number() == 2);

        if !generate_res {
            panic!("node6 can't generate blocks to 2");
        }

        // Although the disconnection of the whitelist is automatically reconnected for node0,
        // the disconnect operation is still needed here to instantly refresh the state of node6 in node0.
        node6.disconnect(node0);

        // Make sure node0 re-connect with node6
        node0.connect(node6);

        // IBD only with outbound/whitelist node
        let rpc_client1 = node1.rpc_client();
        let rpc_client2 = node2.rpc_client();
        let rpc_client3 = node3.rpc_client();
        let rpc_client4 = node4.rpc_client();
        let rpc_client5 = node5.rpc_client();
        let rpc_client6 = node6.rpc_client();

        let is_nodes_ibd_sync = wait_until(10, || {
            let header0 = rpc_client0.get_tip_header();
            let header1 = rpc_client1.get_tip_header();
            let header2 = rpc_client2.get_tip_header();
            let header3 = rpc_client3.get_tip_header();
            let header4 = rpc_client4.get_tip_header();
            let header5 = rpc_client5.get_tip_header();
            let header6 = rpc_client6.get_tip_header();

            header1.inner.number.value() == 0
                && header1 == header5
                && header1 == header4
                && header1 == header3
                && header1 == header2
                && header6.inner.number.value() == 2
                && header0 == header6
        });

        assert!(
            is_nodes_ibd_sync,
            "node 1-5 must not sync with node0, node 6 must sync with node0"
        );
    }
}
