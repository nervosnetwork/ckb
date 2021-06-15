use ckb_jsonrpc_types::{AsEpochNumberWithFraction, ChainInfo};

use crate::util::mining::mine;
use crate::{Node, Spec};

pub struct RpcGetBlockchainInfo;

impl Spec for RpcGetBlockchainInfo {
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let epoch_length = node0.consensus().genesis_epoch_ext().length();

        // get block chain info when tip block is genesis
        let genesis_blockchain_info = node0.rpc_client().get_blockchain_info();
        assert_eq!(
            "ckb_integration_test", genesis_blockchain_info.chain,
            "Blockchain should be 'ckb_integration_test', but got {}",
            genesis_blockchain_info.chain
        );
        assert!(
            genesis_blockchain_info.is_initial_block_download,
            "Block chain should in IBD mode, but not."
        );
        assert_eq!(
            0,
            genesis_blockchain_info.epoch.epoch_number(),
            "Tip Block epoch number should be 0, but got {}",
            genesis_blockchain_info.epoch.epoch_number()
        );
        assert_eq!(
            0,
            genesis_blockchain_info.epoch.epoch_index(),
            "Tip block epoch index should be 0, but got {}",
            genesis_blockchain_info.epoch.epoch_index()
        );
        assert_eq!(
            1,
            genesis_blockchain_info.epoch.epoch_length(),
            "Epoch length of genesis block should be 1, but got {}",
            genesis_blockchain_info.epoch.epoch_length()
        );
        check_median_time(genesis_blockchain_info, node0);

        // mine 1 block, and get block chain info
        mine(node0, 1);
        let blockchain_info = node0.rpc_client().get_blockchain_info();
        // IBD exit
        assert!(
            !blockchain_info.is_initial_block_download,
            "Block chain should out of IBD mode, but not."
        );
        assert_eq!(
            epoch_length,
            blockchain_info.epoch.epoch_length(),
            "Current epoch lenght should be {}, but got {}",
            epoch_length,
            blockchain_info.epoch.epoch_length()
        );
        assert_eq!(
            0,
            blockchain_info.epoch.epoch_number(),
            "Tip block's epoch number should be 0, but got {}",
            blockchain_info.epoch.epoch_number()
        );
        assert_eq!(
            1,
            blockchain_info.epoch.epoch_index(),
            "Tip block's epoch index should be 1, but got {}",
            blockchain_info.epoch.epoch_index()
        );
        // if tip_block_number < median_block_count(default as 37),
        // median_time should be the median block's timestamp from 1 - tip_block_number
        check_median_time(blockchain_info, node0);

        // mine 1 block to make tip_block_number is even
        mine(&node0, 1);
        let blockchain_info = node0.rpc_client().get_blockchain_info();
        assert_eq!(
            2,
            blockchain_info.epoch.epoch_index(),
            "Tip block's epoch index should be 2, but got {}",
            blockchain_info.epoch.epoch_index()
        );
        // the condition of tip_block_number < median_block_count and tip_block_number is even
        check_median_time(blockchain_info, node0);

        // mine epoch_length blocks to make epoch number change
        mine(&node0, epoch_length);
        let blockchain_info = node0.rpc_client().get_blockchain_info();
        assert_eq!(
            1,
            blockchain_info.epoch.epoch_number(),
            "Tip block's epoch number should be 1, but got {}",
            blockchain_info.epoch.epoch_number()
        );
        assert_eq!(
            2,
            blockchain_info.epoch.epoch_index(),
            "Tip block's epoch index should be 2, but got {}",
            blockchain_info.epoch.epoch_index()
        );
        // check median time when tip_block_number > median_block_count
        check_median_time(blockchain_info, node0);
    }
}

fn check_median_time(chain_info: ChainInfo, node: &Node) {
    let tip_block_number = node.get_tip_block_number();
    let median_block_count = node.consensus().median_time_block_count as u64;
    let median_time = if tip_block_number == 0 {
        0
    } else if tip_block_number < median_block_count {
        node.get_block_by_number((tip_block_number + 1) >> 1)
            .timestamp()
    } else {
        node.get_block_by_number(tip_block_number - (median_block_count >> 1))
            .timestamp()
    };

    assert_eq!(
        median_time,
        chain_info.median_time.value(),
        "Median block time should be {}, but got {}",
        median_time,
        chain_info.median_time.value()
    );
}
