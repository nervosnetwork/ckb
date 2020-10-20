use crate::utils::{build_block, build_get_blocks, build_headers, wait_until};
use crate::{Net, Node, Spec};
use ckb_network::SupportProtocols;
use ckb_types::packed::GetBlocks;
use ckb_types::{
    core::{BlockView, TransactionBuilder},
    packed::{self, Byte32, SyncMessage},
    prelude::*,
};
use std::time::Duration;

pub struct ChainContainsInvalidBlock;

impl Spec for ChainContainsInvalidBlock {
    crate::setup!(num_nodes: 3);

    // Case:
    //   1. `bad_node` generate a long chain `CN` contains an invalid block
    //      B[i] is an invalid block.
    //   2. `good_node` synchronizes from `bad_node`. We expect that `good_node`
    //      end synchronizing at B[i-1].
    //   3. `good_node` mines a new block B[i]', i < `CN.length`.
    //   4. `fresh_node` synchronizes from `bad_node` and `good_node`. We expect
    //      that `fresh_node` synchronizes the valid chain.
    fn run(&self, nodes: &mut Vec<Node>) {
        let bad_node = nodes.pop().unwrap();
        let good_node = nodes.pop().unwrap();
        let fresh_node = nodes.pop().unwrap();

        // Build invalid chain on bad_node
        bad_node.generate_blocks(3);
        let invalid_block = bad_node
            .new_block_builder(None, None, None)
            .transaction(TransactionBuilder::default().build())
            .build();
        let invalid_number = invalid_block.header().number();
        let invalid_hash = bad_node.process_block_without_verify(&invalid_block, false);
        bad_node.generate_blocks(3);

        // Start good_node and let it synchronize from bad_node
        good_node.generate_block();
        fresh_node.connect(&good_node);
        good_node.connect_and_wait_ban(&bad_node);
        fresh_node.connect_and_wait_ban(&bad_node);
        assert!(
            wait_until(5, || good_node.get_tip_block_number() >= invalid_number - 1),
            "good_node should synchronize from bad_node 1~{}",
            invalid_number - 1,
        );
        assert!(
            !wait_until(5, || good_node
                .rpc_client()
                .get_block(invalid_hash.clone())
                .is_some()),
            "good_node should not synchronize invalid block {} from bad_node",
            invalid_number,
        );

        // good_node mine the next block
        good_node.generate_block();
        let valid_hash = good_node.get_tip_block().header().hash();
        let valid_number = invalid_number + 1;

        assert!(
            !wait_until(5, || fresh_node.get_tip_block_number() > valid_number),
            "fresh_node should synchronize the valid blocks only",
        );
        assert!(
            wait_until(5, || fresh_node.get_tip_block().header().hash()
                == valid_hash),
            "fresh_node should synchronize the valid blocks only",
        );
    }
}

pub struct ForkContainsInvalidBlock;

impl Spec for ForkContainsInvalidBlock {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        // Build bad forks
        let invalid_number = 4;
        let bad_chain: Vec<BlockView> = {
            let tip_number = invalid_number * 2;
            let bad_node = nodes.pop().unwrap();
            bad_node.generate_blocks(invalid_number - 1);
            let invalid_block = bad_node
                .new_block_builder(None, None, None)
                .transaction(TransactionBuilder::default().build())
                .build();
            bad_node.process_block_without_verify(&invalid_block, false);
            bad_node.generate_blocks(tip_number - invalid_number);
            (1..=bad_node.get_tip_block_number())
                .map(|i| bad_node.get_block_by_number(i))
                .collect()
        };
        let bad_hashes: Vec<Byte32> = bad_chain
            .iter()
            .skip(invalid_number - 1)
            .map(|b| b.hash())
            .collect();

        // Sync headers of bad forks
        let good_node = nodes.pop().unwrap();
        good_node.generate_block();
        let mut net = Net::new(
            self.name(),
            good_node.consensus(),
            vec![SupportProtocols::Sync],
        );
        net.connect(&good_node);
        let headers: Vec<_> = bad_chain.iter().map(|b| b.header()).collect();
        net.send(&good_node, SupportProtocols::Sync, build_headers(&headers));
        let ret = net.should_receive(&good_node, |data| {
            SyncMessage::from_slice(data)
                .map(|message| message.to_enum().item_name() == GetBlocks::NAME)
                .unwrap_or(false)
        });
        assert!(ret, "timeout to wait GetBlocks");

        // Build good chain (good_chain.len < bad_chain.len)
        good_node.generate_blocks(invalid_number + 2);
        let tip_block = good_node.get_tip_block();

        // Sync first part of bad fork which contains an invalid block
        // Good_node cannot detect the invalid block since "block delay verification".
        let (bad_chain1, bad_chain2) = bad_chain.split_at(invalid_number + 1);
        bad_chain1
            .iter()
            .for_each(|block| net.send(&good_node, SupportProtocols::Sync, build_block(block)));
        assert_eq!(good_node.get_tip_block(), tip_block);

        // Sync second part of bad fork.
        // Good_node detect the invalid block when fork.total_difficulty > tip.difficulty
        bad_chain2
            .iter()
            .for_each(|block| net.send(&good_node, SupportProtocols::Sync, build_block(block)));
        let last_hash = bad_chain2.last().map(|b| b.hash()).unwrap();
        assert!(
            !wait_until(10, || good_node
                .rpc_client()
                .get_block(last_hash.clone())
                .is_some()),
            "good_node should keep the good chain",
        );
        assert_eq!(good_node.get_tip_block(), tip_block);

        // Additional testing: request an invalid fork via `GetBlock` should be failed
        net.send(
            &good_node,
            SupportProtocols::Sync,
            build_get_blocks(&bad_hashes),
        );
        let ret = wait_until(10, || {
            if let Ok((_, _, data)) = net.receive_timeout(&good_node, Duration::from_secs(10)) {
                if let Ok(message) = SyncMessage::from_slice(&data) {
                    return message.to_enum().item_name() == packed::SendBlock::NAME;
                }
            }
            false
        });
        assert!(
            !ret,
            "request an invalid fork via GetBlock should be failed"
        );
    }
}
