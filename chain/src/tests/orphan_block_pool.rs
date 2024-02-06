#![allow(dead_code)]
use crate::LonelyBlock;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::core::{BlockBuilder, BlockView, EpochNumberWithFraction, HeaderView};
use ckb_types::prelude::*;
use std::collections::HashSet;
use std::sync::Arc;
use std::thread;

use crate::utils::orphan_block_pool::OrphanBlockPool;

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
        switch: None,
        verify_callback: None,
    }
}

fn assert_leaders_have_children(pool: &OrphanBlockPool) {
    for leader in pool.clone_leaders() {
        let children = pool.remove_blocks_by_parent(&leader);
        assert!(!children.is_empty());
        // `remove_blocks_by_parent` will remove all children from the pool,
        // so we need to put them back here.
        for child in children {
            pool.insert(child);
        }
    }
}

fn assert_blocks_are_sorted(blocks: &[LonelyBlock]) {
    let mut parent_hash = blocks[0].block.header().parent_hash();
    let mut windows = blocks.windows(2);
    // Orphans are sorted in a breadth-first search manner. We iterate through them and
    // check that this is the case.
    // The `parent_or_sibling` may be a sibling or child of current `parent_hash`,
    // and `child_or_sibling` may be a sibling or child of `parent_or_sibling`.
    while let Some([parent_or_sibling, child_or_sibling]) = windows.next() {
        // `parent_or_sibling` is a child of the block with current `parent_hash`.
        // Make `parent_or_sibling`'s parent the current `parent_hash`.
        if parent_or_sibling.block.header().parent_hash() != parent_hash {
            parent_hash = parent_or_sibling.block.header().parent_hash();
        }

        // If `child_or_sibling`'s parent is not the current `parent_hash`, i.e. it is not a sibling of
        // `parent_or_sibling`, then it must be a child of `parent_or_sibling`.
        if child_or_sibling.block.header().parent_hash() != parent_hash {
            assert_eq!(child_or_sibling.block.header().parent_hash(), parent_or_sibling.block.header().hash());
            // Move `parent_hash` forward.
            parent_hash = child_or_sibling.block.header().parent_hash();
        }
    }
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
        let new_block_clone = lonely_block.block().clone();
        let new_block = LonelyBlock {
            block: new_block_clone.clone(),
            switch: None,
            verify_callback: None,
        };
        blocks.push(new_block_clone);

        parent = new_block.block().header();
        pool.insert(new_block);
    }

    let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());

    assert_eq!(
        orphan[0].block.header().parent_hash(),
        consensus.genesis_block().hash()
    );
    assert_blocks_are_sorted(orphan.as_slice());

    let orphan_set: HashSet<_> = orphan.into_iter().map(|b| b.block).collect();
    let blocks_set: HashSet<_> = blocks.into_iter().map(|b| b.to_owned()).collect();
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
        let new_block = lonely_block.block();
        let new_block_clone = LonelyBlock {
            block: Arc::clone(new_block),
            switch: None,
            verify_callback: None,
        };
        pool.insert(new_block_clone);
        header = new_block.header();
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
        let new_block = LonelyBlock {
            block: Arc::clone(lonely_block.block()),
            switch: None,
            verify_callback: None,
        };
        blocks.push(lonely_block);
        parent = new_block.block().header();
        if i % 5 != 0 {
            pool.insert(new_block);
        }
    }
    assert_leaders_have_children(&pool);
    assert_eq!(pool.len(), 15);
    assert_eq!(pool.leaders_len(), 4);

    pool.insert(LonelyBlock {
        block: blocks[5].block().clone(),
        switch: None,
        verify_callback: None,
    });
    assert_leaders_have_children(&pool);
    assert_eq!(pool.len(), 16);
    assert_eq!(pool.leaders_len(), 3);

    pool.insert(LonelyBlock {
        block: blocks[10].block().clone(),
        switch: None,
        verify_callback: None,
    });
    assert_leaders_have_children(&pool);
    assert_eq!(pool.len(), 17);
    assert_eq!(pool.leaders_len(), 2);

    // index 0 doesn't in the orphan pool, so do nothing
    let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
    assert!(orphan.is_empty());
    assert_eq!(pool.len(), 17);
    assert_eq!(pool.leaders_len(), 2);

    pool.insert(LonelyBlock {
        block: blocks[0].block().clone(),
        switch: None,
        verify_callback: None,
    });
    assert_leaders_have_children(&pool);
    assert_eq!(pool.len(), 18);
    assert_eq!(pool.leaders_len(), 2);

    let orphan = pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
    assert_eq!(pool.len(), 3);
    assert_eq!(pool.leaders_len(), 1);

    pool.insert(LonelyBlock {
        block: blocks[15].block().clone(),
        switch: None,
        verify_callback: None,
    });
    assert_leaders_have_children(&pool);
    assert_eq!(pool.len(), 4);
    assert_eq!(pool.leaders_len(), 1);

    let orphan_1 = pool.remove_blocks_by_parent(&blocks[14].block.hash());

    let orphan_set: HashSet<Arc<BlockView>> = orphan
        .into_iter()
        .map(|b| b.block)
        .chain(orphan_1.into_iter().map(|b| b.block))
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
        let lonely_block = LonelyBlock {
            block: Arc::new(new_block),
            switch: None,
            verify_callback: None,
        };
        pool.insert(lonely_block);
    }
    assert_eq!(pool.leaders_len(), 1);

    let v = pool.clean_expired_blocks(20_u64);
    assert_eq!(v.len(), 19);
    assert_eq!(pool.leaders_len(), 0);
}
