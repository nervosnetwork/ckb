use crate::chain::{LonelyBlock, LonelyBlockWithCallback};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::core::{BlockBuilder, BlockView, EpochNumberWithFraction, HeaderView};
use ckb_types::prelude::*;
use std::collections::HashSet;
use std::sync::Arc;
use std::thread;

use crate::orphan_block_pool::OrphanBlockPool;

fn gen_lonely_block(parent_header: &HeaderView) -> LonelyBlock {
    let number = parent_header.number() + 1;
    let block = BlockBuilder::default()
        .parent_hash(parent_header.hash())
        .timestamp(unix_time_as_millis().pack())
        .number(number.pack())
        .epoch(EpochNumberWithFraction::new(number / 1000, number % 1000, 1000).pack())
        .nonce((parent_header.nonce() + 1).pack())
        .build();
    LonelyBlock {
        block: Arc::new(block),
        peer_id: None,
        switch: None,
    }
}

fn gen_lonely_block_with_callback(parent_header: &HeaderView) -> LonelyBlockWithCallback {
    gen_lonely_block(parent_header).without_callback()
}

#[test]
fn test_remove_blocks_by_parent() {
    let consensus = ConsensusBuilder::default().build();
    let block_number = 200;
    let mut blocks = Vec::new();
    let mut parent = consensus.genesis_block().header();
    let pool = OrphanBlockPool::with_capacity(200);
    for _ in 1..block_number {
        let lonely_block = gen_lonely_block(&parent);
        let new_block_clone = lonely_block.clone().without_callback();
        let new_block = lonely_block.without_callback();
        blocks.push(new_block_clone);

        parent = new_block.block().header();
        pool.insert(new_block);
    }

    let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
    let orphan_set: HashSet<_> = orphan.into_iter().map(|b| b.lonely_block.block).collect();
    let blocks_set: HashSet<_> = blocks.into_iter().map(|b| b.lonely_block.block).collect();
    assert_eq!(orphan_set, blocks_set)
}

#[test]
fn test_remove_blocks_by_parent_and_get_block_should_not_deadlock() {
    let consensus = ConsensusBuilder::default().build();
    let pool = OrphanBlockPool::with_capacity(1024);
    let mut header = consensus.genesis_block().header();
    let mut hashes = Vec::new();
    for _ in 1..1024 {
        let lonely_block = gen_lonely_block(&header);
        let new_block = lonely_block.clone().without_callback();
        let new_block_clone = lonely_block.without_callback();
        pool.insert(new_block_clone);
        header = new_block.block().header();
        hashes.push(header.hash());
    }

    let pool_arc1 = Arc::new(pool);
    let pool_arc2 = Arc::clone(&pool_arc1);

    let thread1 = thread::spawn(move || {
        pool_arc1.remove_blocks_by_parent(&consensus.genesis_block().hash());
    });

    for hash in hashes.iter().rev() {
        pool_arc2.get_block(hash);
    }

    thread1.join().unwrap();
}

#[test]
fn test_leaders() {
    let consensus = ConsensusBuilder::default().build();
    let block_number = 20;
    let mut blocks = Vec::new();
    let mut parent = consensus.genesis_block().header();
    let pool = OrphanBlockPool::with_capacity(20);
    for i in 0..block_number - 1 {
        let lonely_block = gen_lonely_block(&parent);
        let new_block = lonely_block.clone().without_callback();
        blocks.push(lonely_block);
        parent = new_block.block().header();
        if i % 5 != 0 {
            pool.insert(new_block);
        }
    }

    assert_eq!(pool.len(), 15);
    assert_eq!(pool.leaders_len(), 4);

    pool.insert(blocks[5].clone().without_callback());
    assert_eq!(pool.len(), 16);
    assert_eq!(pool.leaders_len(), 3);

    pool.insert(blocks[10].clone().without_callback());
    assert_eq!(pool.len(), 17);
    assert_eq!(pool.leaders_len(), 2);

    // index 0 doesn't in the orphan pool, so do nothing
    let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
    assert!(orphan.is_empty());
    assert_eq!(pool.len(), 17);
    assert_eq!(pool.leaders_len(), 2);

    pool.insert(blocks[0].clone().without_callback());
    assert_eq!(pool.len(), 18);
    assert_eq!(pool.leaders_len(), 2);

    let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
    assert_eq!(pool.len(), 3);
    assert_eq!(pool.leaders_len(), 1);

    pool.insert(blocks[15].clone().without_callback());
    assert_eq!(pool.len(), 4);
    assert_eq!(pool.leaders_len(), 1);

    let orphan_1 = pool.remove_blocks_by_parent(&blocks[14].block.hash());

    let orphan_set: HashSet<Arc<BlockView>> = orphan
        .into_iter()
        .map(|b| b.lonely_block.block)
        .chain(orphan_1.into_iter().map(|b| b.lonely_block.block))
        .collect();
    let blocks_set: HashSet<Arc<BlockView>> = blocks.into_iter().map(|b| b.block).collect();
    assert_eq!(orphan_set, blocks_set);
    assert_eq!(pool.len(), 0);
    assert_eq!(pool.leaders_len(), 0);
}

#[test]
fn test_remove_expired_blocks() {
    let consensus = ConsensusBuilder::default().build();
    let block_number = 20;
    let mut parent = consensus.genesis_block().header();
    let pool = OrphanBlockPool::with_capacity(block_number);

    let deprecated = EpochNumberWithFraction::new(10, 0, 10);

    for _ in 1..block_number {
        let new_block = BlockBuilder::default()
            .parent_hash(parent.hash())
            .timestamp(unix_time_as_millis().pack())
            .number((parent.number() + 1).pack())
            .epoch(deprecated.clone().pack())
            .nonce((parent.nonce() + 1).pack())
            .build();

        parent = new_block.header();
        let lonely_block_with_callback = LonelyBlockWithCallback {
            lonely_block: LonelyBlock {
                block: Arc::new(new_block),
                peer_id: None,
                switch: None,
            },
            verify_callback: None,
        };
        pool.insert(lonely_block_with_callback);
    }
    assert_eq!(pool.leaders_len(), 1);

    let v = pool.clean_expired_blocks(20_u64);
    assert_eq!(v.len(), 19);
    assert_eq!(pool.leaders_len(), 0);
}
