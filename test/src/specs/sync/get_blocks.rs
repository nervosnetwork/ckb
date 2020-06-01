use super::utils::wait_get_blocks;
use crate::utils::{build_headers, wait_until};
use crate::{Net, Spec, TestProtocol};
use ckb_network::SupportProtocols;
use ckb_sync::BLOCK_DOWNLOAD_TIMEOUT;
use ckb_types::core::HeaderView;
use log::info;
use std::time::Instant;

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
        net.send(
            SupportProtocols::Sync.protocol_id(),
            pi,
            build_headers(&headers),
        );
        info!("Receive GetBlocks from node1");

        let block_download_timeout_secs = BLOCK_DOWNLOAD_TIMEOUT / 1000;
        let (first, received) = wait_get_blocks_point(block_download_timeout_secs * 2, &net);
        assert!(received, "Should received GetBlocks");
        let (second, received) = wait_get_blocks_point(block_download_timeout_secs * 2, &net);
        assert!(!received, "Should not received GetBlocks");
        let elapsed = second.duration_since(first).as_secs();
        let error_margin = 2;
        assert!(elapsed >= block_download_timeout_secs - error_margin);

        let rpc_client = node1.rpc_client();
        let result = wait_until(10, || {
            let peers = rpc_client.get_peers();
            peers.is_empty()
        });
        if !result {
            panic!("node1 must disconnect net");
        }
    }
}

fn wait_get_blocks_point(secs: u64, net: &Net) -> (Instant, bool) {
    let flag = wait_get_blocks(secs, net);
    (Instant::now(), flag)
}
