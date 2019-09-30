use super::utils::wait_get_blocks;
use crate::utils::build_headers;
use crate::{Net, Spec, TestProtocol};
use ckb_sync::{NetworkProtocol, BLOCK_DOWNLOAD_TIMEOUT};
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

    fn run(&self, net: &mut Net) {
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
        let block_download_timeout_secs = BLOCK_DOWNLOAD_TIMEOUT / 1000;
        let wait_get_blocks_secs = 20;
        assert!(
            block_download_timeout_secs > wait_get_blocks_secs,
            "BLOCK_DOWNLOAD_TIMEOUT should greater than 20 seconds"
        );
        assert!(
            !wait_get_blocks(wait_get_blocks_secs, &net),
            "should not receive GetBlocks"
        );
        let sleep_secs = block_download_timeout_secs - wait_get_blocks_secs + 2;
        thread::sleep(Duration::from_secs(sleep_secs));
        // After about block_download_timeout_secs seconds later
        info!(
            "After {} seconds receive GetBlocks again from node1",
            block_download_timeout_secs + 2
        );
        assert!(wait_get_blocks(10, &net), "timeout to wait GetBlocks");
    }
}
