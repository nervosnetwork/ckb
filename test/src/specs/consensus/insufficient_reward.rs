use crate::global::VENDOR_PATH;
use crate::specs::spec_name;
use crate::{Node, Spec};

use ckb_types::{
    core::{capacity_bytes, Capacity},
    packed::CellOutput,
    prelude::*,
};
use std::convert::Into;

pub struct InsufficientReward;

impl Spec for InsufficientReward {
    fn before_run(&self) -> Vec<Node> {
        let mut node = Node::new(spec_name(self), "node1");

        // modify chain spec
        node.modify_chain_spec(|spec| {
            spec.params.initial_primary_epoch_reward = Capacity::shannons(2000_00000000);
            spec.params.secondary_epoch_reward = Capacity::shannons(100_00000000);
            spec.params.primary_epoch_reward_halving_interval = 2;
            spec.params.epoch_duration_target = 80;
            spec.params.genesis_epoch_length = 20;
        });

        // import vendor data
        let data_path = VENDOR_PATH
            .lock()
            .join("consensus")
            .join("insufficient_reward.json")
            .to_string_lossy()
            .to_string();
        node.import(data_path);

        node.start();
        vec![node]
    }

    // Case: block which reward is insufficient could not be submitted
    //    1. submit block with insufficient reward in current epoch should failed;
    //    2. submit block with empty reward should success.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let new_block_builder = node.new_block_builder(None, None, None);

        // build a block with insufficient reward
        let output = CellOutput::new_builder()
            .capacity(capacity_bytes!(1).pack())
            .lock(Default::default())
            .build();
        let cellbase = new_block_builder.clone().build().transactions()[0]
            .as_advanced_builder()
            .output(output)
            .build();
        let new_block = new_block_builder
            .clone()
            .set_transactions(vec![cellbase])
            .build();
        let result = node
            .rpc_client()
            .submit_block("".to_owned(), new_block.data().into());

        assert!(
            result
                .expect_err("invalid block submit failed")
                .to_string()
                .contains("Block(Cellbase(InvalidOutputQuantity))"),
            "Insufficient reward block should be submitted failed, but not"
        );

        // build a block with empty reward
        let new_block = new_block_builder.build();
        let cellbase = &new_block.transactions()[0];
        let result = node
            .rpc_client()
            .submit_block("".to_owned(), new_block.data().into());

        assert!(
            cellbase.outputs().is_empty(),
            "Cellbase output should be empty"
        );
        assert!(
            result.is_ok(),
            "Empty reward block should be submitted successfully, but not"
        )
    }

    // export data
    // fn run(&self, nodes: &mut Vec<Node>) {
    //     let node = &mut nodes[0];
    //     let hashes = node.generate_blocks(100);

    //     for hash in hashes {
    //         let blk: BlockView = node.rpc_client().get_block(hash).unwrap().into();
    //         let cellbase = &blk.transactions()[0];
    //         info!(
    //             "block number {} outputs_capacity {} epoch {}",
    //             blk.number(),
    //             cellbase.outputs_capacity().unwrap(),
    //             blk.epoch().number()
    //         );
    //     }

    //     node.stop();
    //     node.export("${backup}".to_string());
    // }
}
