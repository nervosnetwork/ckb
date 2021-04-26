use crate::node::waiting_for_sync;
use crate::util::mining::{mine, out_ibd_mode};
use crate::utils::{now_ms, sleep, wait_until};
use crate::{Node, Spec};
use ckb_logger::info;
use ckb_types::prelude::*;
use std::time::Duration;

pub struct RelayTooNewBlock;

impl Spec for RelayTooNewBlock {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        info!("run relay too new block");
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];
        out_ibd_mode(&nodes);

        node1.connect(node0);
        let future = Duration::from_secs(6_000).as_millis() as u64;
        let too_new_block = node0
            .new_block_builder(None, None, None)
            .timestamp((now_ms() + future).pack())
            .build();

        let _too_new_hash = node0.process_block_without_verify(&too_new_block, true);
        // sync node0 node2
        mine(node2, 2);
        node2.connect(node0);
        waiting_for_sync(&[node0, node2]);

        sleep(15); // GET_HEADERS_TIMEOUT 15s
        mine(&node0, 1);
        let (rpc_client0, rpc_client1) = (node0.rpc_client(), node1.rpc_client());
        let ret = wait_until(20, || {
            let header0 = rpc_client0.get_tip_header();
            let header1 = rpc_client1.get_tip_header();
            header0 == header1 && header1.inner.number.value() == 4
        });
        assert!(ret, "Node1 should not ban Node0",);
    }
}
