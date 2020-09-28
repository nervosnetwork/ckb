use crate::{Node, Spec};
use ckb_types::core::{BlockView, EpochNumberWithFraction};
use ckb_types::prelude::*;

// Convention:
//   main-block: a block on the main fork
//   main-uncle: an uncle block be embedded in the main fork
//   fork-block: a block on a side fork
//   fork-uncle: an uncle block be embedded in a side fork

pub struct UncleInheritFromForkBlock;

impl Spec for UncleInheritFromForkBlock {
    crate::setup!(num_nodes: 2);

    // Case: A uncle inherited from a fork-block in side fork is invalid, because that breaks
    //       the uncle rule "B1's parent is either B2's ancestor or embedded in B2
    //       or its ancestors as an uncle"
    //    1. Build a chain which embedded `uncle` as an uncle;
    //    2. Force reorg, so that the parent of `uncle` become fork-block;
    //    3. Submit block with `uncle`, which is inherited from a fork-block, should be failed;
    //    4. Add all the fork-blocks as uncle blocks into the chain and re-submit block with
    //       `uncle` should be success

    fn run(&self, nodes: &mut Vec<Node>) {
        let target_node = &nodes[0];
        let feed_node = &nodes[1];

        let uncle = construct_uncle(target_node);

        let longer_fork = (0..=target_node.get_tip_block_number()).map(|_| {
            let block = feed_node.new_block(None, None, None);
            feed_node.submit_block(&block);
            block
        });
        longer_fork.for_each(|block| {
            target_node.submit_block(&block);
        });

        let block = target_node
            .new_block_builder(None, None, None)
            .set_uncles(vec![uncle.as_uncle()])
            .build();
        let ret = target_node
            .rpc_client()
            .submit_block("0".to_owned(), block.data().into());
        assert!(
            ret.is_err(),
            "Submit block with uncle inherited from a fork-block should be failed, but got {:?}",
            ret
        );
        let err = ret.unwrap_err();
        assert!(
            err.to_string().contains("DescendantLimit"),
            "The result should contain 'DescendantLimit', but got {:?}",
            err
        );

        until_no_uncles_left(target_node);
        let block = target_node
            .new_block_builder(None, None, None)
            .set_uncles(vec![uncle.as_uncle()])
            .build();
        target_node.submit_block(&block);
    }
}

pub struct UncleInheritFromForkUncle;

impl Spec for UncleInheritFromForkUncle {
    crate::setup!(num_nodes: 2);

    // Case: A uncle inherited from a fork-uncle in side fork is invalid, because that breaks
    //       the uncle rule "B1's parent is either B2's ancestor or embedded in B2
    //       or its ancestors as an uncle"
    //    1. Build a chain which embedded `uncle_parent` as an uncle;
    //    2. Force reorg, so that `uncle_parent` become a fork-uncle;
    //    3. Submit block with `uncle`, which is inherited from fork-uncle `uncle_parent`,
    //       should be failed
    //    4. Add all the fork-blocks as uncle blocks into the chain and now re-submit block with
    //       `uncle_child` should be success
    fn run(&self, nodes: &mut Vec<Node>) {
        let target_node = &nodes[0];
        let feed_node = &nodes[1];

        let uncle_parent = construct_uncle(target_node);
        target_node.submit_block(&uncle_parent);

        let uncle_child = uncle_parent
            .as_advanced_builder()
            .number((uncle_parent.number() + 1).pack())
            .parent_hash(uncle_parent.hash())
            .timestamp((uncle_parent.timestamp() + 1).pack())
            .epoch(
                {
                    let parent_epoch = uncle_parent.epoch();
                    let epoch_number = parent_epoch.number();
                    let epoch_index = parent_epoch.index();
                    let epoch_length = parent_epoch.length();
                    EpochNumberWithFraction::new(epoch_number, epoch_index + 1, epoch_length)
                }
                .pack(),
            )
            .build();

        let longer_fork = (0..=target_node.get_tip_block_number()).map(|_| {
            let block = feed_node.new_block(None, None, None);
            feed_node.submit_block(&block);
            block
        });
        longer_fork.for_each(|block| {
            target_node.submit_block(&block);
        });

        let block = target_node
            .new_block_builder(None, None, None)
            .set_uncles(vec![uncle_child.as_uncle()])
            .build();
        let ret = target_node
            .rpc_client()
            .submit_block("0".to_owned(), block.data().into());
        assert!(
            ret.is_err(),
            "Submit block with uncle inherited from a fork-uncle should be failed, but got {:?}",
            ret
        );
        let err = ret.unwrap_err();
        assert!(
            err.to_string().contains("DescendantLimit"),
            "The result should contain 'DescendantLimit', but got {:?}",
            err
        );

        until_no_uncles_left(target_node);
        let block = target_node
            .new_block_builder(None, None, None)
            .set_uncles(vec![uncle_child.as_uncle()])
            .build();
        target_node.submit_block(&block);
    }
}

pub struct PackUnclesIntoEpochStarting;

impl Spec for PackUnclesIntoEpochStarting {
    // Case: Miner should not add uncles into the epoch starting
    //    1. Chain grow until CURRENT_EPOCH_END - 1 and submit uncle;
    //    2. Expect the next mining block(CURRENT_EPOCH_END) contains the uncle;
    //    3. Submit CURRENT_EPOCH_END block with empty uncles;
    //    4. Expect the next mining block(NEXT_EPOCH_START) not contains uncle.

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let uncle = construct_uncle(node);
        let next_epoch_start = {
            let current_epoch = node.rpc_client().get_current_epoch();
            current_epoch.start_number.value() + current_epoch.length.value()
        };
        let current_epoch_end = next_epoch_start - 1;

        node.generate_blocks((current_epoch_end - node.get_tip_block_number() - 1) as usize);
        node.submit_block(&uncle);

        let block = node.new_block(None, None, None);
        assert_eq!(
            1,
            block.uncles().into_iter().count(),
            "Current_epoch_end block should contain the uncle"
        );

        let block_with_empty_uncles = block.as_advanced_builder().set_uncles(vec![]).build();
        node.submit_block(&block_with_empty_uncles);

        let block = node.new_block(None, None, None);
        assert_eq!(
            0,
            block.uncles().into_iter().count(),
            "Next_epoch_start block should not contain the uncle"
        );
    }
}

// Convenient way to construct an uncle block
fn construct_uncle(node: &Node) -> BlockView {
    node.generate_block(); // Ensure exit IBD mode
    let uncle = node.construct_uncle();
    node.generate_block();

    uncle
}

// Convenient way to add all the fork-blocks as uncles into the main chain
fn until_no_uncles_left(node: &Node) {
    loop {
        let block = node.new_block(None, None, None);
        if block.uncles().into_iter().count() == 0 {
            break;
        }
        node.submit_block(&block);
    }
}
