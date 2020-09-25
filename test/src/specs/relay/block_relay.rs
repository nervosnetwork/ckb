use crate::node::{connect_all, exit_ibd_mode};
use crate::utils::{now_ms, sleep, wait_until};
use crate::{Node, Spec};
use ckb_types::prelude::*;
use log::info;
use std::time::Duration;

pub struct BlockRelayBasic;

impl Spec for BlockRelayBasic {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        exit_ibd_mode(nodes);
        connect_all(nodes);

        let hash = nodes[0].generate_block();
        let synced = wait_until(10, || {
            nodes
                .iter()
                .all(|node| node.rpc_client().get_block(hash.clone()).is_some())
        });
        assert!(synced, "Block should be relayed from node0 to others");
    }
}

pub struct RelayTooNewBlock;

impl Spec for RelayTooNewBlock {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        info!("run relay too new block");
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];
        exit_ibd_mode(nodes);

        node1.connect(node0);
        let future = Duration::from_secs(6_000).as_millis() as u64;
        let too_new_block = node0
            .new_block_builder(None, None, None)
            .timestamp((now_ms() + future).pack())
            .build();

        let _too_new_hash = node0.process_block_without_verify(&too_new_block, true);
        // sync node0 node2
        node2.generate_blocks(2);
        node2.connect(node0);
        node2.waiting_for_sync(node0, node2.get_tip_block_number());

        sleep(15); // GET_HEADERS_TIMEOUT 15s
        node0.generate_block();
        let (rpc_client0, rpc_client1) = (node0.rpc_client(), node1.rpc_client());
        let ret = wait_until(20, || {
            let header0 = rpc_client0.get_tip_header();
            let header1 = rpc_client1.get_tip_header();
            header0 == header1 && header1.inner.number.value() == 4
        });
        assert!(ret, "Node1 should not ban Node0",);
    }
}
