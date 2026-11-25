use crate::util::log_monitor::monitor_log_until_expected_show;
use crate::utils::{build_block, build_get_blocks, build_headers, tweaked_duration, wait_until};
use crate::{Net, Node, Spec};
use ckb_network::SupportProtocols;
use ckb_types::{
    core::{BlockView, TransactionBuilder},
    packed::{self, Byte32, GetBlocks, SyncMessage},
    prelude::*,
};
use std::collections::HashSet;
use std::time::{Duration, Instant};

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
        bad_node.mine(3);
        let invalid_block = bad_node
            .new_block_builder(None, None, None)
            .transaction(TransactionBuilder::default().build())
            .build();
        let invalid_number = invalid_block.header().number();
        let invalid_hash = bad_node.process_block_without_verify(&invalid_block, false);
        bad_node.mine(3);

        // Start good_node and let it synchronize from bad_node
        good_node.mine(1);
        fresh_node.connect(&good_node);
        good_node.connect_and_wait_ban(&bad_node);
        fresh_node.connect_and_wait_ban(&bad_node);
        assert!(
            wait_until(5, || good_node.get_tip_block_number() >= invalid_number - 1),
            "good_node should synchronize from bad_node 1~{}",
            invalid_number - 1,
        );
        assert!(
            good_node
                .rpc_client()
                .get_block(invalid_hash.clone())
                .is_none(),
            "good_node should not synchronize invalid block {invalid_number} from bad_node"
        );

        // good_node mine the next block
        good_node.mine(1);
        let valid_hash = good_node.get_tip_block().header().hash();
        let valid_number = good_node.get_tip_block_number();
        assert_eq!(
            valid_number, invalid_number,
            "good_node mined valid replacement at invalid_number"
        );

        assert!(
            wait_until(5, || fresh_node.get_tip_block().header().hash()
                == valid_hash),
            "fresh_node should synchronize the valid blocks only",
        );
        assert_eq!(
            fresh_node.get_tip_block_number(),
            valid_number,
            "fresh_node should stay at valid_number"
        );
        assert!(
            fresh_node.rpc_client().get_block(invalid_hash).is_none(),
            "fresh_node should not store invalid block from bad_node"
        );
        assert!(
            !wait_until(1, || fresh_node.get_tip_block_number() > valid_number),
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
            bad_node.mine(invalid_number - 1);
            let invalid_block = bad_node
                .new_block_builder(None, None, None)
                .transaction(TransactionBuilder::default().build())
                .build();
            bad_node.process_block_without_verify(&invalid_block, false);
            bad_node.mine(tip_number - invalid_number);
            (1..=bad_node.get_tip_block_number())
                .map(|i| bad_node.get_block_by_number(i))
                .collect()
        };
        let bad_hashes: Vec<Byte32> = bad_chain
            .iter()
            .skip(invalid_number as usize - 1)
            .map(|b| b.hash())
            .collect();

        // Sync headers of bad forks
        let good_node = nodes.pop().unwrap();
        good_node.mine(1);
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
        good_node.mine(invalid_number + 2);
        let tip_block = good_node.get_tip_block();

        // Sync first part of bad fork which contains an invalid block
        // Good_node cannot detect the invalid block since "block delay verification".
        let (bad_chain1, bad_chain2) = bad_chain.split_at(invalid_number as usize + 1);
        bad_chain1
            .iter()
            .for_each(|block| net.send(&good_node, SupportProtocols::Sync, build_block(block)));
        assert_eq!(good_node.get_tip_block(), tip_block);

        // Sync second part of bad fork.
        // Good_node detects the invalid block when fork.total_difficulty > tip.difficulty,
        // rejects the fork, and bans the peer.
        bad_chain2
            .iter()
            .for_each(|block| net.send(&good_node, SupportProtocols::Sync, build_block(block)));
        let last_hash = bad_chain2.last().map(|b| b.hash()).unwrap();
        let banned = wait_until(10, || {
            !good_node.rpc_client().get_banned_addresses().is_empty()
                && good_node.rpc_client().get_peers().is_empty()
        });
        assert!(
            banned,
            "good_node should reject the invalid fork and ban the peer"
        );
        assert!(
            good_node.rpc_client().get_block(last_hash).is_none(),
            "good_node should keep the good chain",
        );
        assert_eq!(good_node.get_tip_block(), tip_block);

        // Additional testing: reconnect a fresh peer after clearing the ban and verify that
        // requesting the invalid fork via `GetBlocks` does not return any invalid fork blocks.
        good_node.rpc_client().clear_banned_addresses();
        let mut net2 = Net::new(
            self.name(),
            good_node.consensus(),
            vec![SupportProtocols::Sync],
        );
        net2.connect(&good_node);
        let tip_hash = tip_block.hash();
        net2.send(
            &good_node,
            SupportProtocols::Sync,
            build_get_blocks(&bad_hashes),
        );
        net2.send(
            &good_node,
            SupportProtocols::Sync,
            build_get_blocks(std::slice::from_ref(&tip_hash)),
        );

        let mut received_blocks = HashSet::new();
        let deadline = Instant::now() + tweaked_duration(10);
        while !received_blocks.contains(&tip_hash) {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let Ok((_, _, data)) = net2.receive_timeout(&good_node, remaining) else {
                break;
            };
            if let Ok(message) = SyncMessage::from_compatible_slice(&data)
                && let packed::SyncMessageUnion::SendBlock(send_block) = message.to_enum()
            {
                received_blocks.insert(send_block.block().calc_header_hash());
            }
        }
        assert!(
            received_blocks.contains(&tip_hash),
            "good_node should still serve valid tip block via GetBlocks"
        );
        let last_bad_hash = bad_hashes.last().unwrap();
        assert!(
            monitor_log_until_expected_show(
                &good_node,
                0,
                10,
                &format!("Ignoring get_block {last_bad_hash} request"),
            )
            .is_some(),
            "good_node should log ignoring invalid fork GetBlocks request"
        );
        while let Ok((_, _, data)) = net2.receive_timeout(&good_node, Duration::from_millis(300)) {
            if let Ok(message) = SyncMessage::from_compatible_slice(&data)
                && let packed::SyncMessageUnion::SendBlock(send_block) = message.to_enum()
            {
                received_blocks.insert(send_block.block().calc_header_hash());
            }
        }
        assert!(
            bad_hashes.iter().all(|h| !received_blocks.contains(h)),
            "request an invalid fork via GetBlocks should not return invalid blocks"
        );
    }
}
