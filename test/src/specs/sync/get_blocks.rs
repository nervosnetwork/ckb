use crate::utils::{build_headers, wait_get_blocks};
use crate::{Net, Spec, TestProtocol};
use ckb_sync::NetworkProtocol;
use ckb_types::core::HeaderView;
use log::info;
use std::thread;
use std::time::Duration;

pub struct GetBlocksTimeout;

impl Spec for GetBlocksTimeout {
    crate::name!("get_blocks_timeout");

    crate::setup!(
        connect_all: false,
        num_nodes: 2,
        protocols: vec![TestProtocol::sync()],
    );

    fn run(&self, mut net: Net) {
        let node1 = net.nodes.pop().unwrap();
        let node2 = net.nodes.pop().unwrap();
        node1.generate_blocks(1);
        node2.generate_blocks(20);

        let headers: Vec<HeaderView> = (1..=node2.get_tip_block_number())
            .map(|i| node2.get_header_by_number(i))
            .collect();

        net.connect(&node1);
        let (pi, _, _) = net.receive();
        info!("Send Headers to node1");
        net.send(NetworkProtocol::SYNC.into(), pi, build_headers(&headers));
        info!("Receive GetBlocks from node1");
        assert!(wait_get_blocks(10, &net), "timeout to wait GetBlocks");
        for _ in 0..2 {
            assert!(!wait_get_blocks(10, &net), "should not receive GetBlocks");
        }
        thread::sleep(Duration::from_secs(12));
        // After about 32 seconds later
        info!("After 32 seconds receive GetBlocks again from node1");
        assert!(wait_get_blocks(10, &net), "timeout to wait GetBlocks");
    }
}
