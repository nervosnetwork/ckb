use super::utils::wait_get_blocks;
use crate::utils::{build_block, build_get_blocks, build_headers, wait_until};
use crate::{Net, Spec, TestProtocol};
use ckb_sync::NetworkProtocol;
use ckb_types::{
    core::{BlockView, TransactionBuilder},
    packed::{self, Byte32, SyncMessage},
    prelude::*,
};
use std::time::Duration;

pub struct ChainContainsInvalidBlock;

impl Spec for ChainContainsInvalidBlock {
    crate::name!("chain_contains_invalid_block");

    crate::setup!(
        connect_all: false,
        num_nodes: 3,
        protocols: vec![TestProtocol::sync()],
    );

    // Case:
    //   1. `bad_node` generate a long chain `CN` contains a invalid block
    //      B[i] is an invalid block.
    //   2. `good_node` synchronizes from `bad_node`. We expect that `good_node`
    //      end synchronizing at B[i-1].
    //   3. `good_node` mines a new block B[i]', i < `CN.length`.
    //   4. `fresh_node` synchronizes from `bad_node` and `good_node`. We expect
    //      that `fresh_node` synchronizes the valid chain.
    fn run(&self, net: &mut Net) {
        let bad_node = net.nodes.pop().unwrap();
        let good_node = net.nodes.pop().unwrap();
        let fresh_node = net.nodes.pop().unwrap();

        // Build invalid chain on bad_node
        bad_node.generate_blocks(3);
        let invalid_block = bad_node
            .new_block_builder(None, None, None)
            .transaction(TransactionBuilder::default().build())
            .build();
        let invalid_number = invalid_block.header().number();
        let invalid_hash = bad_node.process_block_without_verify(&invalid_block);
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
        let valid_hash = good_node.get_tip_block().header().hash().clone();
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
    crate::name!("fork_contains_invalid_block");

    crate::setup!(
        connect_all: false,
        num_nodes: 2,
        protocols: vec![TestProtocol::sync()],
    );

    fn run(&self, net: &mut Net) {
        // Build bad forks
        let invalid_number = 4;
        let bad_chain: Vec<BlockView> = {
            let tip_number = invalid_number * 2;
            let bad_node = net.nodes.pop().unwrap();
            bad_node.generate_blocks(invalid_number - 1);
            let invalid_block = bad_node
                .new_block_builder(None, None, None)
                .transaction(TransactionBuilder::default().build())
                .build();
            bad_node.process_block_without_verify(&invalid_block);
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
        let good_node = net.nodes.pop().unwrap();
        good_node.generate_block();
        net.connect(&good_node);
        let (pi, _, _) = net.receive();
        let headers: Vec<_> = bad_chain.iter().map(|b| b.header().clone()).collect();
        net.send(NetworkProtocol::SYNC.into(), pi, build_headers(&headers));
        assert!(wait_get_blocks(10, &net), "timeout to wait GetBlocks",);

        // Build good chain (good_chain.len < bad_chain.len)
        good_node.generate_blocks(invalid_number + 2);
        let tip_block = good_node.get_tip_block();

        // Sync first part of bad fork which contains an invalid block
        // Good_node cannot detect the invalid block since "block delay verification".
        let (bad_chain1, bad_chain2) = bad_chain.split_at(invalid_number + 1);
        bad_chain1
            .iter()
            .for_each(|block| net.send(NetworkProtocol::SYNC.into(), pi, build_block(block)));
        let last_hash = bad_chain1.last().map(|b| b.hash()).unwrap();
        assert!(
            wait_until(10, || good_node
                .rpc_client()
                .get_fork_block(last_hash.clone())
                .is_some()),
            "good_node should store the fork blocks even it contains invalid blocks",
        );
        assert_eq!(good_node.get_tip_block(), tip_block);

        // Sync second part of bad fork.
        // Good_node detect the invalid block when fork.total_difficulty > tip.difficulty
        bad_chain2
            .iter()
            .for_each(|block| net.send(NetworkProtocol::SYNC.into(), pi, build_block(block)));
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
            NetworkProtocol::SYNC.into(),
            pi,
            build_get_blocks(&bad_hashes),
        );
        let ret = wait_until(10, || {
            if let Ok((_, _, data)) = net.receive_timeout(Duration::from_secs(10)) {
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
