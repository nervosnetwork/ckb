use crate::{
    utils::{now_ms, sleep, wait_until},
    TestProtocol,
};
use crate::{Net, Spec};
use ckb_types::prelude::*;
use log::info;
use std::time::Duration;

pub struct BlockRelayBasic;

impl Spec for BlockRelayBasic {
    crate::name!("block_relay_basic");

    crate::setup!(num_nodes: 3);

    fn run(&self, net: &mut Net) {
        net.exit_ibd_mode();
        let node0 = net.node(0);
        let node1 = net.node(1);
        let node2 = net.node(2);

        info!("Generate new block on node1");
        let hash = node1.generate_block();

        let rpc_client = node0.rpc_client();
        let ret = wait_until(10, || rpc_client.get_block(hash.clone()).is_some());
        assert!(ret, "Block should be relayed to node0");

        let rpc_client = node2.rpc_client();
        let ret = wait_until(10, || rpc_client.get_block(hash.clone()).is_some());
        assert!(ret, "Block should be relayed to node2");
    }
}

pub struct RelayTooNewBlock;

impl Spec for RelayTooNewBlock {
    crate::name!("relay_too_new_block");

    crate::setup!(
        num_nodes: 3,
        connect_all: false,
        protocols: vec![TestProtocol::relay()],
    );

    fn run(&self, net: &mut Net) {
        info!("run relay too new block");
        let node0 = net.node(0);
        let node1 = net.node(1);
        let node2 = net.node(2);
        net.exit_ibd_mode();

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
