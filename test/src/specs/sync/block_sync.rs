use crate::node::exit_ibd_mode;
use crate::utils::{
    build_block, build_compact_block, build_get_blocks, build_header, new_block_with_template,
    now_ms, sleep, wait_until,
};
use crate::{Net, Node, Spec};
use ckb_jsonrpc_types::ChainInfo;
use ckb_network::{bytes::Bytes, SupportProtocols};
use ckb_types::{
    core::BlockView,
    packed::{self, Byte32, SyncMessage},
    prelude::*,
};
use log::info;
use std::time::Duration;

pub struct BlockSyncFromOne;

impl Spec for BlockSyncFromOne {
    crate::setup!(num_nodes: 2);

    // NOTE: ENSURE node0 and nodes1 is in genesis state.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let (rpc_client0, rpc_client1) = (node0.rpc_client(), node1.rpc_client());
        assert_eq!(0, rpc_client0.get_tip_block_number());
        assert_eq!(0, rpc_client1.get_tip_block_number());

        (0..3).for_each(|_| {
            node0.generate_block();
        });

        node1.connect(node0);

        let ret = wait_until(10, || {
            let header0 = rpc_client0.get_tip_header();
            let header1 = rpc_client1.get_tip_header();
            header0 == header1 && header0.inner.number.value() == 3
        });
        assert!(
            ret,
            "Node0 and node1 should sync with each other until same tip chain",
        );
    }
}

pub struct BlockSyncWithUncle;

impl Spec for BlockSyncWithUncle {
    crate::setup!(num_nodes: 2);

    // Case: Sync a block with uncle
    fn run(&self, nodes: &mut Vec<Node>) {
        let target = &nodes[0];
        let node1 = &nodes[1];
        exit_ibd_mode(nodes);

        let new_builder = node1.new_block_builder(None, None, None);
        let new_block1 = new_builder.clone().nonce(0.pack()).build();
        let new_block2 = new_builder.nonce(1.pack()).build();

        node1.submit_block(&new_block1);
        node1.submit_block(&new_block2);

        let uncle = if node1.get_tip_block() == new_block1 {
            new_block2.as_uncle()
        } else {
            new_block1.as_uncle()
        };

        let block_builder = node1.new_block_builder(None, None, None);

        node1.submit_block(&block_builder.set_uncles(vec![uncle.clone()]).build());

        target.connect(node1);
        target.waiting_for_sync(node1, 3);

        // check whether node panic
        assert!(target.rpc_client().get_block(uncle.hash()).is_none());
    }
}

pub struct BlockSyncForks;

impl Spec for BlockSyncForks {
    crate::setup!(num_nodes: 3);

    // NOTE: ENSURE node0 and nodes1 is in genesis state.
    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];
        let (rpc_client0, rpc_client1, rpc_client2) =
            (node0.rpc_client(), node1.rpc_client(), node2.rpc_client());
        assert_eq!(0, rpc_client0.get_tip_block_number());
        assert_eq!(0, rpc_client1.get_tip_block_number());
        assert_eq!(0, rpc_client2.get_tip_block_number());

        build_forks(node0, &[2, 0, 0, 0, 0, 0, 0, 0, 0]);
        build_forks(node1, &[1, 0, 0, 0, 0, 0, 0, 0, 0]);
        build_forks(node2, &[5, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

        let tip0 = rpc_client0.get_tip_header();
        let tip1 = rpc_client1.get_tip_header();
        assert_eq!(tip0.inner.number, tip1.inner.number);
        assert_ne!(tip0.hash, tip1.hash);

        // Connect node0 and node1, so that they can sync with each other
        node0.connect(node1);
        let ret = wait_until(5, || {
            let header0 = rpc_client0.get_tip_header();
            let header1 = rpc_client1.get_tip_header();
            header0 == header1
        });
        assert!(
            !ret,
            "Node0 and node1 sync but still have respective tips as first-received policy",
        );

        let tip2 = rpc_client2.get_tip_header();
        assert_eq!(tip0.inner.number.value() + 1, tip2.inner.number.value());

        // Connect node0 and node2, so that they can sync with each other
        node0.connect(node2);
        let ret = wait_until(10, || {
            let header0 = rpc_client0.get_tip_header();
            let header2 = rpc_client2.get_tip_header();
            header0 == header2
        });
        assert!(
            ret,
            "Node0 and node2 should sync with each other until same tip chain",
        );

        for number in 1u64..tip2.inner.number.into() {
            let block0 = rpc_client0.get_block_by_number(number);
            let block2 = rpc_client2.get_block_by_number(number);
            assert_eq!(
                block0, block2,
                "nodes should have same best chain after synchronizing",
            );
        }
        let info00: ChainInfo = rpc_client0.get_blockchain_info();
        let info22: ChainInfo = rpc_client2.get_blockchain_info();

        assert_eq!(info00.median_time, info22.median_time);
    }
}

pub struct BlockSyncDuplicatedAndReconnect;

impl Spec for BlockSyncDuplicatedAndReconnect {
    // Case: Sync a header, sync a duplicated header, reconnect and sync a duplicated header
    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let rpc_client = node.rpc_client();
        exit_ibd_mode(nodes);
        let mut net = Net::new(self.name(), node.consensus(), vec![SupportProtocols::Sync]);
        net.connect(node);

        // Sync a new header to `node`, `node` should send back a corresponding GetBlocks message
        let block = node.new_block(None, None, None);
        sync_header(&net, node, &block);

        should_receive_get_blocks_message(&net, node, block.hash());

        // Sync duplicated header again, `node` should discard the duplicated one.
        // So we will not receive any response messages
        sync_header(&net, node, &block);
        assert!(
            net.receive_timeout(node, Duration::new(10, 0)).is_err(),
            "node should discard duplicated sync headers",
        );

        // Disconnect and reconnect node, and then sync the same header
        // `node` should send back a corresponding GetBlocks message
        let ctrl = net.controller();
        let peers_num = ctrl.connected_peers().len();
        let peer = ctrl.connected_peers()[peers_num - 1].clone();
        ctrl.remove_node(&peer.1.peer_id);
        wait_until(5, || {
            rpc_client.get_peers().is_empty() && ctrl.connected_peers().is_empty()
        });

        net.connect(node);
        sync_header(&net, node, &block);

        should_receive_get_blocks_message(&net, node, block.hash());

        // Sync corresponding block entity, `node` should accept the block as tip block
        sync_block(&net, node, &block);
        let hash = block.header().hash();
        wait_until(10, || rpc_client.get_tip_header().hash == hash.unpack());
    }
}

pub struct BlockSyncOrphanBlocks;

impl Spec for BlockSyncOrphanBlocks {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        exit_ibd_mode(nodes);

        // Generate some blocks from node1
        let mut blocks: Vec<BlockView> = (1..=5)
            .map(|_| {
                let block = node1.new_block(None, None, None);
                node1.submit_block(&block);
                block
            })
            .collect();

        let mut net = Net::new(self.name(), node0.consensus(), vec![SupportProtocols::Sync]);
        net.connect(node0);
        let rpc_client = node0.rpc_client();
        let tip_number = rpc_client.get_tip_block_number();

        // Send headers to node0, keep blocks body
        blocks.iter().for_each(|block| {
            sync_header(&net, node0, block);
        });

        // Wait for block fetch timer
        should_receive_get_blocks_message(&net, node0, blocks.last().unwrap().hash());

        // Skip the next block, send the rest blocks to node0
        let first = blocks.remove(0);
        blocks.into_iter().for_each(|block| {
            sync_block(&net, node0, &block);
        });
        let ret = wait_until(5, || rpc_client.get_tip_block_number() > tip_number);
        assert!(!ret, "node0 should stay the same");

        // Send that skipped first block to node0
        sync_block(&net, node0, &first);
        let ret = wait_until(10, || rpc_client.get_tip_block_number() > tip_number + 2);
        assert!(ret, "node0 should grow up");
    }
}

/// test case:
/// 1. generate 1-17 block
/// 2. sync 1-16 header to node
/// 3. sync 2-16 block to node
/// 4. sync 1 block to node
/// 5. relay 17 block to node
pub struct BlockSyncRelayerCollaboration;

impl Spec for BlockSyncRelayerCollaboration {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        exit_ibd_mode(nodes);

        // Generate some blocks from node1
        let mut blocks: Vec<BlockView> = (1..=17)
            .map(|_| {
                let block = node1.new_block(None, None, None);
                node1.submit_block(&block);
                block
            })
            .collect();

        let mut net = Net::new(
            self.name(),
            node0.consensus(),
            vec![SupportProtocols::Sync, SupportProtocols::Relay],
        );
        net.connect(node0);
        let rpc_client = node0.rpc_client();
        let tip_number = rpc_client.get_tip_block_number();

        let last = blocks.pop().unwrap();

        // Send headers to node0, keep blocks body
        blocks.iter().for_each(|block| {
            sync_header(&net, node0, block);
        });

        // Wait for block fetch timer
        should_receive_get_blocks_message(&net, node0, blocks.last().unwrap().hash());

        // Skip the next block, send the rest blocks to node0
        let first = blocks.remove(0);
        blocks.into_iter().for_each(|block| {
            sync_block(&net, node0, &block);
        });

        let ret = wait_until(5, || rpc_client.get_tip_block_number() > tip_number);
        assert!(!ret, "node0 should stay the same");

        sync_block(&net, node0, &first);
        net.send(node0, SupportProtocols::Relay, build_compact_block(&last));

        let ret = wait_until(10, || rpc_client.get_tip_block_number() >= tip_number + 17);
        log::info!("{}", rpc_client.get_tip_block_number());
        assert!(ret, "node0 should grow up");
    }
}

pub struct BlockSyncNonAncestorBestBlocks;

impl Spec for BlockSyncNonAncestorBestBlocks {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        exit_ibd_mode(nodes);

        // By picking blocks this way, we ensure that block a and b has
        // the same difficulty, but different hash. So
        // later when we sync the header of block a with node0, node0's
        // global shared best header will be updated, but the tip will stay
        // unchanged. Then we can connect node0 with node1, node1 will provide
        // a better chain that is not the known best's ancestor.
        let a = node0.new_block(None, None, None);
        // This ensures a and b are different
        let b = a
            .data()
            .as_advanced_builder()
            .timestamp((a.timestamp() + 1).pack())
            .build();
        assert_ne!(a.hash(), b.hash());
        node1.submit_block(&b);

        let mut net = Net::new(self.name(), node0.consensus(), vec![SupportProtocols::Sync]);
        net.connect(node0);
        // With a header synced to node0, node0 should have a new best header
        // but tip is not updated yet.
        sync_header(&net, node0, &a);

        node1.connect(node0);
        let (rpc_client0, rpc_client1) = (node0.rpc_client(), node1.rpc_client());
        let ret = wait_until(20, || {
            let header0 = rpc_client0.get_tip_header();
            let header1 = rpc_client1.get_tip_header();
            header0 == header1 && header0.inner.number.value() == 2
        });
        assert!(
            ret,
            "Node0 and node1 should sync with each other until same tip chain",
        );
    }
}

pub struct RequestUnverifiedBlocks;

impl Spec for RequestUnverifiedBlocks {
    crate::setup!(num_nodes: 3);

    // Case:
    //   1. `target_node` maintains an unverified fork
    //   2. Expect that when other peers request `target_node` for the blocks on the unverified
    //      fork(referred to as fork-blocks), `target_node` should discard the request because
    //     these fork-blocks are unverified yet or verified failed.
    fn run(&self, nodes: &mut Vec<Node>) {
        let target_node = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];
        exit_ibd_mode(nodes);

        let main_chain = build_forks(node1, &[0; 6]);
        let fork_chain = build_forks(node2, &[1; 5]);
        assert!(main_chain.len() > fork_chain.len());

        // Submit `main_chain` before `fork_chain`, to make `target_node` marks `fork_chain`
        // unverified because of delay-verify
        main_chain.iter().for_each(|block| {
            target_node.submit_block(block);
        });
        fork_chain.iter().for_each(|block| {
            target_node.submit_block(block);
        });
        let main_hashes: Vec<_> = main_chain.iter().map(|block| block.hash()).collect();
        let fork_hashes: Vec<_> = fork_chain.iter().map(|block| block.hash()).collect();

        // Request for the blocks on `main_chain` and `fork_chain`. We should only receive the
        // `main_chain` blocks
        let mut net = Net::new(
            self.name(),
            target_node.consensus(),
            vec![SupportProtocols::Sync],
        );
        net.connect(target_node);
        sync_get_blocks(&net, target_node, &main_hashes);
        sync_get_blocks(&net, target_node, &fork_hashes);

        let mut received = Vec::new();
        while let Ok((_, _, data)) = net.receive_timeout(target_node, Duration::from_secs(10)) {
            let message = SyncMessage::from_slice(&data).unwrap();
            if let packed::SyncMessageUnionReader::SendBlock(reader) = message.as_reader().to_enum()
            {
                received.push(reader.block().calc_header_hash());
            }
        }
        assert!(
            main_hashes.iter().all(|hash| received.contains(hash)),
            "Expect receiving all of the main_chain blocks: {:?}, actual: {:?}",
            main_hashes,
            received,
        );
        assert!(
            fork_hashes.iter().all(|hash| !received.contains(hash)),
            "Expect not receiving any of the fork_chain blocks: {:?}, actual: {:?}",
            fork_hashes,
            received,
        );
    }
}

pub struct SyncTooNewBlock;

impl Spec for SyncTooNewBlock {
    crate::setup!(num_nodes: 3);

    fn run(&self, nodes: &mut Vec<Node>) {
        info!("run sync too new block");
        let node0 = &nodes[0];
        let node1 = &nodes[1];
        let node2 = &nodes[2];
        exit_ibd_mode(nodes);

        let future = Duration::from_secs(6_000).as_millis() as u64;

        for _ in 0..3 {
            let too_new_block = node0
                .new_block_builder(None, None, None)
                .timestamp((now_ms() + future).pack())
                .build();

            // node1 sync node1 with too new block
            let _too_new_hash = node0.process_block_without_verify(&too_new_block, false);
        }

        let (rpc_client0, rpc_client1) = (node0.rpc_client(), node1.rpc_client());
        node1.connect(node0);

        // sync node0 node2
        node2.generate_blocks(6);
        node2.connect(node0);
        node2.waiting_for_sync(node0, node2.get_tip_block_number());
        node2.disconnect(node0);

        sleep(15); // GET_HEADERS_TIMEOUT 15s
        node0.generate_block();
        let ret = wait_until(20, || {
            let header0 = rpc_client0.get_tip_header();
            let header1 = rpc_client1.get_tip_header();
            header0 == header1 && header1.inner.number.value() == 8
        });
        assert!(ret, "Node1 should not ban Node0",);
    }
}

fn build_forks(node: &Node, offsets: &[u64]) -> Vec<BlockView> {
    let rpc_client = node.rpc_client();
    let mut blocks = Vec::with_capacity(offsets.len());
    for offset in offsets.iter() {
        let mut template = rpc_client.get_block_template(None, None, None);
        template.current_time = (template.current_time.value() + offset).into();
        let block = new_block_with_template(template);
        node.submit_block(&block);

        blocks.push(block);
    }
    blocks
}

fn sync_header(net: &Net, node: &Node, block: &BlockView) {
    net.send(node, SupportProtocols::Sync, build_header(&block.header()));
}

fn sync_block(net: &Net, node: &Node, block: &BlockView) {
    net.send(node, SupportProtocols::Sync, build_block(block));
}

fn sync_get_blocks(net: &Net, node: &Node, hashes: &[Byte32]) {
    net.send(node, SupportProtocols::Sync, build_get_blocks(hashes));
}

fn should_receive_get_blocks_message(net: &Net, node: &Node, last_block_hash: Byte32) {
    let ret = net.should_receive(node, |data: &Bytes| {
        SyncMessage::from_slice(&data)
            .map(|message| match message.to_enum() {
                packed::SyncMessageUnion::GetBlocks(get_blocks) => {
                    let block_hashes = get_blocks.block_hashes();
                    block_hashes.get(block_hashes.len() - 1).unwrap() == last_block_hash
                }
                _ => false,
            })
            .unwrap_or(false)
    });
    assert!(ret, "Test node should receive GetBlocks message from node");
}
