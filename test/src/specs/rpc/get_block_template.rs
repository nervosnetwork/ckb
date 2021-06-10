use crate::util::mining::mine;
use crate::{Node, Spec};
use ckb_jsonrpc_types::AsEpochNumberWithFraction;
use ckb_types::prelude::*;

pub struct RpcGetBlockTemplate;

impl Spec for RpcGetBlockTemplate {
    // This case about block template just involves the fileds come from consensus,
    // the other fields with more logic calculation will included in other cases.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let default_bytes_limit = node0.consensus().max_block_bytes;
        let default_cycles_limit = node0.consensus().max_block_cycles;
        let default_block_version = node0.consensus().block_version;
        let epoch_length = node0.consensus().genesis_epoch_ext().length();

        // get block template when tip block is genesis
        let genesis_block_template = node0.rpc_client().get_block_template(None, None, None);
        assert_eq!(
            default_bytes_limit,
            genesis_block_template.bytes_limit.value(),
            "Default bytes limit should be {}, but got {}",
            default_bytes_limit,
            genesis_block_template.bytes_limit.value(),
        );
        assert_eq!(
            default_cycles_limit,
            genesis_block_template.cycles_limit.value(),
            "Default cycles limit should be {}, but got {}",
            default_cycles_limit,
            genesis_block_template.cycles_limit.value(),
        );
        assert_eq!(
            default_block_version,
            genesis_block_template.version.value(),
            "Default block version should be {}, but got {}",
            default_block_version,
            genesis_block_template.version.value(),
        );
        let next_block_number = node0.get_tip_block_number() + 1;
        assert_eq!(
            next_block_number,
            genesis_block_template.number.value(),
            "Next block number should be {}, but got {}",
            next_block_number,
            genesis_block_template.number.value()
        );
        let parent_block_hash = node0.get_tip_block().hash();
        assert_eq!(
            parent_block_hash,
            genesis_block_template.parent_hash.pack(),
            "Parent block hash should be {}, but got {}",
            parent_block_hash,
            genesis_block_template.parent_hash.pack()
        );
        assert_eq!(
            0,
            genesis_block_template.epoch.epoch_number(),
            "Next block epoch should be 0, but got {}",
            genesis_block_template.epoch.epoch_number()
        );

        // mine until met last block of this epoch
        mine(node0, epoch_length - 1);
        let block_template = node0.rpc_client().get_block_template(None, None, None);
        let next_block_number = node0.get_tip_block_number() + 1;
        assert_eq!(
            next_block_number,
            block_template.number.value(),
            "Next block number should be {}, but got {}",
            next_block_number,
            block_template.number.value()
        );
        // the epoch number of block template should +1
        assert_eq!(
            1,
            block_template.epoch.epoch_number(),
            "Next block epoch should be 1, but got {}",
            block_template.number.value()
        );
        // the epoch index should start from 0
        assert_eq!(
            0,
            block_template.epoch.epoch_index(),
            "Next block epoch index should be 0, but got {}",
            block_template.epoch.epoch_index()
        );

        // get block template with arguments
        // proposal limit arg will be tested in other cases
        let test_bytes_limit = default_bytes_limit >> 1;
        let test_version = 42;
        let block_template =
            node0
                .rpc_client()
                .get_block_template(Some(test_bytes_limit), None, Some(test_version));
        assert_eq!(
            test_bytes_limit,
            block_template.bytes_limit.value(),
            "Bytes limit should be {}, but got {}",
            test_bytes_limit,
            block_template.bytes_limit.value()
        );
        // block version should be the minium between version from consensus and arguments
        assert_eq!(
            default_block_version.min(test_version),
            block_template.version.value(),
            "Block version should be {}, but got {}",
            default_block_version.min(test_version),
            block_template.version.value(),
        );
    }
}
