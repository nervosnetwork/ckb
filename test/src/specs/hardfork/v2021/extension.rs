use crate::{
    node::waiting_for_sync,
    util::{
        check::{assert_epoch_should_be, assert_submit_block_fail, assert_submit_block_ok},
        mining::{mine, mine_until_epoch, mine_until_out_bootstrap_period},
    },
    utils::wait_until,
};
use crate::{Node, Spec};
use ckb_logger::{info, trace};
use ckb_types::prelude::*;

const GENESIS_EPOCH_LENGTH: u64 = 10;

const ERROR_UNKNOWN_FIELDS: &str = "Invalid: Block(UnknownFields(";
const ERROR_EMPTY_EXT: &str = "Invalid: Block(EmptyBlockExtension(";
const ERROR_MAX_LIMIT: &str = "Invalid: Block(ExceededMaximumBlockExtensionBytes(";

pub struct CheckBlockExtension;

impl Spec for CheckBlockExtension {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        {
            let node = &nodes[0];
            let epoch_length = GENESIS_EPOCH_LENGTH;

            mine_until_out_bootstrap_period(node);

            assert_epoch_should_be(node, 1, 2, epoch_length);
            {
                info!("CKB v2019, empty extension field is failed");
                test_extension_via_size(node, Some(0), Err(ERROR_UNKNOWN_FIELDS));
            }
            {
                info!("CKB v2019, overlength extension field is failed");
                test_extension_via_size(node, Some(97), Err(ERROR_UNKNOWN_FIELDS));
            }
            for size in &[1, 16, 32, 64, 96] {
                info!("CKB v2019, {}-bytes extension field is failed", size);
                test_extension_via_size(node, Some(*size), Err(ERROR_UNKNOWN_FIELDS));
            }
            assert_epoch_should_be(node, 1, 2, epoch_length);
            {
                info!("CKB v2019, no extension field is passed");
                test_extension_via_size(node, None, Ok(()));
            }
            assert_epoch_should_be(node, 1, 3, epoch_length);

            mine_until_epoch(node, 1, epoch_length - 2, epoch_length);
            {
                info!("CKB v2019, empty extension field is failed (boundary)");
                test_extension_via_size(node, Some(0), Err(ERROR_UNKNOWN_FIELDS));
            }
            {
                info!("CKB v2019, overlength extension field is failed (boundary)");
                test_extension_via_size(node, Some(97), Err(ERROR_UNKNOWN_FIELDS));
            }
            for size in &[1, 16, 32, 64, 96] {
                info!(
                    "CKB v2019, {}-bytes extension field is failed (boundary)",
                    size
                );
                test_extension_via_size(node, Some(*size), Err(ERROR_UNKNOWN_FIELDS));
            }
            {
                info!("CKB v2019, no extension field is passed (boundary)");
                test_extension_via_size(node, None, Ok(()));
            }
            assert_epoch_should_be(node, 1, epoch_length - 1, epoch_length);

            {
                info!("CKB v2021, empty extension field is failed (boundary)");
                test_extension_via_size(node, Some(0), Err(ERROR_EMPTY_EXT));
            }
            {
                info!("CKB v2021, overlength extension field is failed (boundary)");
                test_extension_via_size(node, Some(97), Err(ERROR_MAX_LIMIT));
            }
            assert_epoch_should_be(node, 1, epoch_length - 1, epoch_length);
            for size in &[1, 16, 32, 64, 96] {
                info!(
                    "CKB v2021, {}-bytes extension field is passed (boundary)",
                    size
                );
                test_extension_via_size(node, Some(*size), Ok(()));
            }
            {
                info!("CKB v2021, no extension field is passed (boundary)");
                test_extension_via_size(node, None, Ok(()));
            }
            assert_epoch_should_be(node, 2, 5, epoch_length);

            mine_until_epoch(node, 4, 0, epoch_length);
            {
                info!("CKB v2021, empty extension field is failed");
                test_extension_via_size(node, Some(0), Err(ERROR_EMPTY_EXT));
            }
            {
                info!("CKB v2021, overlength extension field is failed");
                test_extension_via_size(node, Some(97), Err(ERROR_MAX_LIMIT));
            }
            assert_epoch_should_be(node, 4, 0, epoch_length);
            for size in &[1, 16, 32, 64, 96] {
                info!("CKB v2021, {}-bytes extension field is passed", size);
                test_extension_via_size(node, Some(*size), Ok(()));
            }
            {
                info!("CKB v2021, no extension field is passed");
                test_extension_via_size(node, None, Ok(()));
            }
            assert_epoch_should_be(node, 4, 6, epoch_length);
        }

        {
            info!("test sync blocks for two nodes");
            let node0 = &nodes[0];
            let node1 = &nodes[1];

            let rpc_client0 = node0.rpc_client();
            let rpc_client1 = node1.rpc_client();

            node1.connect(node0);
            let ret = wait_until(30, || {
                let number0 = rpc_client0.get_tip_block_number();
                let number1 = rpc_client1.get_tip_block_number();
                trace!("block number: node0: {}, node1: {}", number0, number1);
                number0 == number1
            });
            assert!(ret, "node1 should get same tip header with node0");
        }

        {
            info!("test reload data from store after restart the node");
            let node0 = &mut nodes[0];
            node0.stop();
            node0.start();
        }

        {
            info!("test sync blocks for all nodes");
            let node0 = &nodes[0];
            let node1 = &nodes[1];
            let node2 = &nodes[2];

            let rpc_client0 = node0.rpc_client();
            let rpc_client2 = node2.rpc_client();

            node1.connect(node0);
            node2.connect(node0);
            let ret = wait_until(30, || {
                let header0 = rpc_client0.get_tip_header();
                let header2 = rpc_client2.get_tip_header();
                header0 == header2
            });
            assert!(ret, "node2 should get same tip header with node0");

            mine(node2, 5);

            info!("test sync blocks");
            waiting_for_sync(nodes);
        }
    }

    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.permanent_difficulty_in_dummy = Some(true);
        spec.params.genesis_epoch_length = Some(GENESIS_EPOCH_LENGTH);
        if let Some(mut switch) = spec.params.hardfork.as_mut() {
            switch.rfc_pr_0224 = Some(2);
        }
    }
}

fn test_extension_via_size(node: &Node, size: Option<usize>, result: Result<(), &'static str>) {
    let block = node
        .new_block_builder(None, None, None)
        .extension(size.map(|s| vec![0u8; s].pack()))
        .build();
    if let Err(errmsg) = result {
        assert_submit_block_fail(node, &block, errmsg);
    } else {
        assert_submit_block_ok(node, &block);
    }
}
