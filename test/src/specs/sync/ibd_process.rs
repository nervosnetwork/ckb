use crate::util::log_monitor::monitor_log_until_expected_count;
use crate::utils::wait_until;
use crate::{Node, Spec};
use ckb_logger::info;
use std::fs;

pub struct IBDProcess;

impl Spec for IBDProcess {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        nodes
            .iter()
            .for_each(|node| node.wait_find_unverified_blocks_finished());
        info!("Running IBD process");

        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];

        // The node's outbound connection does not retain the peer which in the ibd state
        node0.mine(1);
        // will never connect
        node0.connect_uncheck(node1);
        node0.connect_uncheck(node2);

        assert!(
            monitor_log_until_expected_count(node0, 0, 20, "getheader with ibd peer", 2),
            "node0 should detect and disconnect both outbound IBD peers"
        );

        let rpc_client0 = node0.rpc_client();
        let rpc_client1 = node1.rpc_client();
        let rpc_client2 = node2.rpc_client();
        let is_connect_peer_num_eq_0 = wait_until(20, || {
            rpc_client0.get_peers().is_empty()
                && rpc_client1.get_peers().is_empty()
                && rpc_client2.get_peers().is_empty()
        });

        if !is_connect_peer_num_eq_0 {
            panic!("refuse to connect fail");
        }

        // IBD only with outbound/whitelist node
        let is_nodes_ibd_sync = wait_until(20, || {
            let header1 = rpc_client1.get_tip_header();
            let header2 = rpc_client2.get_tip_header();

            header1.inner.number.value() == 0 && header1 == header2
        });

        assert!(is_nodes_ibd_sync, "node 1-2 must not sync with node0");

        node1.connect(node0);
        node2.connect(node0);

        let is_node_sync = wait_until(20, || {
            let header1 = rpc_client1.get_tip_header();
            let header2 = rpc_client2.get_tip_header();
            header1 == header2 && header1.inner.number.value() == 1
        });

        assert!(is_node_sync, "node 1-2 must sync with node0");
    }
}

pub struct IBDProcessWithWhiteList;

impl Spec for IBDProcessWithWhiteList {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        info!("Running IBD process with whitelist");

        let restart_log_offset = {
            nodes[0].stop();

            // whitelist will be connected on outbound on node start
            let node2_address = nodes[2].p2p_address();
            nodes[0].modify_app_config(move |config| {
                config.network.whitelist_peers = vec![node2_address.parse().unwrap()];
            });
            let offset = fs::metadata(nodes[0].log_path())
                .map(|m| m.len())
                .unwrap_or(0);
            nodes[0].start();
            offset
        };

        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];
        let node0_id = node0.node_id();
        let node2_id = node2.node_id();
        let rpc_client0 = node0.rpc_client();
        let rpc_client1 = node1.rpc_client();
        let rpc_client2 = node2.rpc_client();

        // 1. Wait for node0's startup dial to its whitelisted IBD peer (node2) to
        //    complete the IBD handshake and stay connected despite node2 being in IBD.
        assert!(
            monitor_log_until_expected_count(
                node0,
                restart_log_offset,
                20,
                "getheader with ibd peer",
                1
            ),
            "node0 should observe whitelisted node2 in IBD"
        );
        assert!(
            wait_until(20, || {
                let peers0 = rpc_client0.get_peers();
                let peers2 = rpc_client2.get_peers();
                peers0.len() == 1
                    && peers0[0].node_id == node2_id
                    && peers2.iter().any(|p| p.node_id == node0_id)
            }),
            "node0 should retain its whitelisted outbound connection to node2 during IBD"
        );

        // 2. Dial non-whitelisted IBD peer (node1) and deterministically verify that
        //    node0 observes node1 in IBD, evicts node1 on both sides, and keeps node2.
        node0.connect_uncheck(node1);
        assert!(
            monitor_log_until_expected_count(
                node0,
                restart_log_offset,
                20,
                "getheader with ibd peer",
                2
            ),
            "node0 should detect and reject non-whitelisted outbound IBD peer node1"
        );
        assert!(
            wait_until(20, || {
                let peers0 = rpc_client0.get_peers();
                peers0.len() == 1
                    && peers0[0].node_id == node2_id
                    && rpc_client1.get_peers().is_empty()
            }),
            "node0 should evict non-whitelisted IBD peer node1 while keeping whitelisted node2"
        );

        // 3. Bring node2 out of IBD, disconnect cleanly, and reconnect so node0 lifts
        //    its suspended-sync state on node2 and syncs to height 2.
        node2.mine(2);
        assert_eq!(node2.get_tip_block_number(), 2);

        node2.disconnect(node0);
        node0.connect(node2);

        // IBD only with outbound/whitelist node
        let is_nodes_ibd_sync = wait_until(20, || {
            let header0 = rpc_client0.get_tip_header();
            let header1 = rpc_client1.get_tip_header();
            let header2 = rpc_client2.get_tip_header();

            header1.inner.number.value() == 0
                && header1 != header2
                && header2.inner.number.value() == 2
                && header0 == header2
        });

        assert!(
            is_nodes_ibd_sync,
            "node 1 must not sync with node0, node2 must sync with node0"
        );
        assert!(
            rpc_client1.get_peers().is_empty(),
            "node1 must remain disconnected"
        );
    }
}
