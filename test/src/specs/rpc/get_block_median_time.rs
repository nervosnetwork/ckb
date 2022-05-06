use crate::{Node, Spec};
use ckb_types::packed::Byte32;
use std::collections::HashMap;

pub struct RpcGetBlockMedianTime;

impl Spec for RpcGetBlockMedianTime {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let median_time_block_count = node.consensus().median_time_block_count() as u64;
        let mut expected_median_time_map = HashMap::new();

        assert_eq!(node.get_tip_block_number(), 0);
        let median_time = node.rpc_client().get_blockchain_info().median_time.value();
        expected_median_time_map.insert(0u64, median_time);
        for number in 1..=median_time_block_count * 2 {
            node.mine(1);
            let median_time = node.rpc_client().get_blockchain_info().median_time.value();
            expected_median_time_map.insert(number, median_time);
        }

        for number in 0..=median_time_block_count * 2 {
            let block_hash = node.get_header_by_number(number).hash();
            let actual = node
                .rpc_client()
                .get_block_median_time(block_hash)
                .unwrap()
                .value();
            let expected = *expected_median_time_map.get(&number).unwrap();
            assert_eq!(
                expected, actual,
                "block #{} median time does not match, expected = {}, actual = {}",
                number, expected, actual,
            )
        }

        // test get_block_median_time with unknown block hash
        assert_eq!(
            None,
            node.rpc_client().get_block_median_time(Byte32::zero())
        );
    }
}
