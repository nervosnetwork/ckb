use crate::utils::{build_headers, wait_until};
use crate::{Net, Node, Spec};
use ckb_network::SupportProtocols;
use ckb_sync::BLOCK_DOWNLOAD_TIMEOUT;
use ckb_types::core::HeaderView;
use ckb_types::packed::{GetBlocks, SyncMessage};
use ckb_types::prelude::*;
use failure::_core::time::Duration;
use log::info;
use std::time::Instant;

pub struct GetBlocksTimeout;

impl Spec for GetBlocksTimeout {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node1 = nodes.pop().unwrap();
        let node2 = nodes.pop().unwrap();
        node1.generate_blocks(1);
        node2.generate_blocks(20);

        let headers: Vec<HeaderView> = (1..=node2.get_tip_block_number())
            .map(|i| node2.get_header_by_number(i))
            .collect();

        let mut net = Net::new(self.name(), node1.consensus(), vec![SupportProtocols::Sync]);
        net.connect(&node1);
        info!("Send Headers to node1");
        net.send(&node1, SupportProtocols::Sync, build_headers(&headers));
        info!("Receive GetBlocks from node1");

        let block_download_timeout_secs = BLOCK_DOWNLOAD_TIMEOUT / 1000;
        let (first, received) =
            wait_get_blocks_point(&net, &node1, block_download_timeout_secs * 2);
        assert!(received, "Should received GetBlocks");
        let (second, received) =
            wait_get_blocks_point(&net, &node1, block_download_timeout_secs * 2);
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

fn wait_get_blocks_point(net: &Net, node: &Node, secs: u64) -> (Instant, bool) {
    let flag = wait_until(secs, || {
        if let Ok((_, _, data)) = net.receive_timeout(node, Duration::from_secs(1)) {
            if let Ok(message) = SyncMessage::from_slice(&data) {
                return message.to_enum().item_name() == GetBlocks::NAME;
            }
        }
        false
    });
    (Instant::now(), flag)
}
