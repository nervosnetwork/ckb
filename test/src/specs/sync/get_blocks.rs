use crate::util::mining::mine;
use crate::utils::{build_headers, wait_until};
use crate::{Net, Node, Spec};
use ckb_constant::sync::{BLOCK_DOWNLOAD_TIMEOUT, INIT_BLOCKS_IN_TRANSIT_PER_PEER};
use ckb_logger::info;
use ckb_network::SupportProtocols;
use ckb_types::{core::HeaderView, packed, prelude::*};
use std::time::{Duration, Instant};

pub struct GetBlocksTimeout;

impl Spec for GetBlocksTimeout {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node1 = nodes.pop().unwrap();
        let node2 = nodes.pop().unwrap();

        mine(&node1, 1);
        mine(&node2, INIT_BLOCKS_IN_TRANSIT_PER_PEER as u64 + 20);

        let headers: Vec<HeaderView> = (1..=node2.get_tip_block_number())
            .map(|i| node2.get_header_by_number(i))
            .collect();
        let expected_hash = headers[INIT_BLOCKS_IN_TRANSIT_PER_PEER - 1].hash();

        let mut net = Net::new(self.name(), node1.consensus(), vec![SupportProtocols::Sync]);
        net.connect(&node1);
        info!("Send Headers to node1");
        net.send(&node1, SupportProtocols::Sync, build_headers(&headers));
        info!("Receive GetBlocks from node1");

        let block_download_timeout_secs = BLOCK_DOWNLOAD_TIMEOUT / 1000;
        let received = wait_get_blocks_point(
            &net,
            &node1,
            block_download_timeout_secs * 2,
            INIT_BLOCKS_IN_TRANSIT_PER_PEER,
        );
        assert!(received.is_some(), "Should received GetBlocks");
        let (count, last_hash) = received.unwrap();
        assert!(
            count == INIT_BLOCKS_IN_TRANSIT_PER_PEER,
            "Should received only {} GetBlocks",
            INIT_BLOCKS_IN_TRANSIT_PER_PEER
        );
        assert!(
            expected_hash == last_hash,
            "The last hash of GetBlocks should be {:#x} but got {:#x}",
            expected_hash,
            last_hash,
        );

        let received = wait_get_blocks_point(&net, &node1, block_download_timeout_secs * 2, 1);
        assert!(
            received.is_some(),
            "in the case of sparse connections, even if download times out, net should continue to receive GetBlock requests"
        );

        let rpc_client = node1.rpc_client();
        let result = wait_until(10, || {
            let peers = rpc_client.get_peers();
            !peers.is_empty()
        });
        if !result {
            panic!("node1 must not disconnect net");
        }
    }
}

fn wait_get_blocks_point(
    net: &Net,
    node: &Node,
    secs: u64,
    expected_count: usize,
) -> Option<(usize, packed::Byte32)> {
    let mut count = 0;
    let instant = Instant::now();
    let mut last_hash = None;
    while instant.elapsed() < Duration::from_secs(secs) {
        if let Ok((_, _, data)) = net.receive_timeout(node, Duration::from_secs(1)) {
            if let Ok(message) = packed::SyncMessage::from_slice(&data) {
                if let packed::SyncMessageUnion::GetBlocks(inner) = message.to_enum() {
                    count += inner.block_hashes().len();
                    if let Some(hash) = inner.block_hashes().into_iter().last() {
                        last_hash = Some(hash);
                    }
                    if count >= expected_count {
                        break;
                    }
                }
            }
        }
    }
    last_hash.map(|hash| (count, hash))
}
