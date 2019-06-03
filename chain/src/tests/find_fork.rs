use crate::chain::{ChainService, ForkChanges};
use crate::tests::util::gen_block;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::extras::{BlockExt, DaoStats, DEFAULT_ACCUMULATED_RATE};
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::NotifyService;
use ckb_shared::shared::SharedBuilder;
use ckb_traits::ChainProvider;
use faketime::unix_time_as_millis;
use numext_fixed_uint::U256;
use std::collections::HashSet;
use std::iter::FromIterator;
use std::sync::Arc;

// 0--1--2--3--4
// \
//  \
//   1--2--3--4
#[test]
fn test_find_fork_case1() {
    let builder = SharedBuilder::<MemoryKeyValueDB>::new();
    let shared = builder.consensus(Consensus::default()).build().unwrap();
    let notify = NotifyService::default().start::<&str>(None);
    let mut chain_service = ChainService::new(shared.clone(), notify);
    let genesis = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    let mut fork1: Vec<Block> = Vec::new();
    let mut fork2: Vec<Block> = Vec::new();

    let mut parent = genesis.clone();
    for _ in 0..4 {
        let new_block = gen_block(&parent, U256::from(100u64), vec![], vec![], vec![]);
        fork1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    let mut parent = genesis.clone();
    for _ in 0..3 {
        let new_block = gen_block(&parent, U256::from(90u64), vec![], vec![], vec![]);
        fork2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    // fork1 total_difficulty 400
    for blk in &fork1 {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    // fork2 total_difficulty 270
    for blk in &fork2 {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    let tip_number = { shared.lock_chain_state().tip_number() };

    // fork2 total_difficulty 470
    let new_block = gen_block(&parent, U256::from(200u64), vec![], vec![], vec![]);
    fork2.push(new_block.clone());

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: U256::zero(),
        total_uncles_count: 0,
        // if txs in parent is invalid, txs in block is also invalid
        txs_verified: None,
        dao_stats: DaoStats {
            accumulated_rate: DEFAULT_ACCUMULATED_RATE,
            accumulated_capacity: 0,
        },
    };

    let mut fork = ForkChanges::default();

    chain_service.find_fork(&mut fork, tip_number, &new_block, ext);

    let detached_blocks: HashSet<Block> = HashSet::from_iter(fork1.into_iter());
    let attached_blocks: HashSet<Block> = HashSet::from_iter(fork2.into_iter());
    assert_eq!(
        detached_blocks,
        HashSet::from_iter(fork.detached_blocks.iter().cloned())
    );
    assert_eq!(
        attached_blocks,
        HashSet::from_iter(fork.attached_blocks.iter().cloned())
    );
}

// 0--1--2--3--4
//    \
//     \
//      2--3--4
#[test]
fn test_find_fork_case2() {
    let builder = SharedBuilder::<MemoryKeyValueDB>::new();
    let shared = builder.consensus(Consensus::default()).build().unwrap();
    let notify = NotifyService::default().start::<&str>(None);
    let mut chain_service = ChainService::new(shared.clone(), notify);

    let genesis = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    let mut fork1: Vec<Block> = Vec::new();
    let mut fork2: Vec<Block> = Vec::new();

    let mut parent = genesis.clone();
    for _ in 0..4 {
        let new_block = gen_block(&parent, U256::from(100u64), vec![], vec![], vec![]);
        fork1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    let mut parent = fork1[0].header().to_owned();
    for _ in 0..2 {
        let new_block = gen_block(&parent, U256::from(90u64), vec![], vec![], vec![]);
        fork2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    // fork1 total_difficulty 400
    for blk in &fork1 {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    // fork2 total_difficulty 280
    for blk in &fork2 {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    let tip_number = { shared.lock_chain_state().tip_number() };

    // fork2 total_difficulty 480
    let difficulty = parent.difficulty().to_owned();
    let new_block = gen_block(
        &parent,
        difficulty + U256::from(200u64),
        vec![],
        vec![],
        vec![],
    );
    fork2.push(new_block.clone());

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: U256::zero(),
        total_uncles_count: 0,
        // if txs in parent is invalid, txs in block is also invalid
        txs_verified: None,
        dao_stats: DaoStats {
            accumulated_rate: DEFAULT_ACCUMULATED_RATE,
            accumulated_capacity: 0,
        },
    };

    let mut fork = ForkChanges::default();

    chain_service.find_fork(&mut fork, tip_number, &new_block, ext);

    let detached_blocks: HashSet<Block> = HashSet::from_iter(fork1[1..].iter().cloned());
    let attached_blocks: HashSet<Block> = HashSet::from_iter(fork2.into_iter());
    assert_eq!(
        detached_blocks,
        HashSet::from_iter(fork.detached_blocks.iter().cloned())
    );
    assert_eq!(
        attached_blocks,
        HashSet::from_iter(fork.attached_blocks.iter().cloned())
    );
}

// 0--1--2--3
// \                _ fork
//  \             /
//   1--2--3--4--5--6
#[test]
fn test_find_fork_case3() {
    let builder = SharedBuilder::<MemoryKeyValueDB>::new();
    let shared = builder.consensus(Consensus::default()).build().unwrap();
    let notify = NotifyService::default().start::<&str>(None);
    let mut chain_service = ChainService::new(shared.clone(), notify);

    let genesis = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    let mut fork1: Vec<Block> = Vec::new();
    let mut fork2: Vec<Block> = Vec::new();

    let mut parent = genesis.clone();
    for _ in 0..3 {
        let new_block = gen_block(&parent, U256::from(80u64), vec![], vec![], vec![]);
        fork1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    let mut parent = genesis.clone();
    for _ in 0..5 {
        let new_block = gen_block(&parent, U256::from(40u64), vec![], vec![], vec![]);
        fork2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    // fork1 total_difficulty 240
    for blk in &fork1 {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    // fork2 total_difficulty 200
    for blk in &fork2 {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    let tip_number = { shared.lock_chain_state().tip_number() };

    // fork2 total_difficulty 300
    let new_block = gen_block(&parent, U256::from(100u64), vec![], vec![], vec![]);
    fork2.push(new_block.clone());

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: U256::zero(),
        total_uncles_count: 0,
        // if txs in parent is invalid, txs in block is also invalid
        txs_verified: None,
        dao_stats: DaoStats {
            accumulated_rate: DEFAULT_ACCUMULATED_RATE,
            accumulated_capacity: 0,
        },
    };
    let mut fork = ForkChanges::default();

    chain_service.find_fork(&mut fork, tip_number, &new_block, ext);

    let detached_blocks: HashSet<Block> = HashSet::from_iter(fork1.into_iter());
    let attached_blocks: HashSet<Block> = HashSet::from_iter(fork2.into_iter());
    assert_eq!(
        detached_blocks,
        HashSet::from_iter(fork.detached_blocks.iter().cloned())
    );
    assert_eq!(
        attached_blocks,
        HashSet::from_iter(fork.attached_blocks.iter().cloned())
    );
}

// 0--1--2--3--4--5
// \        _ fork
//  \     /
//   1--2--3
#[test]
fn test_find_fork_case4() {
    let builder = SharedBuilder::<MemoryKeyValueDB>::new();
    let shared = builder.consensus(Consensus::default()).build().unwrap();
    let notify = NotifyService::default().start::<&str>(None);
    let mut chain_service = ChainService::new(shared.clone(), notify);

    let genesis = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    let mut fork1: Vec<Block> = Vec::new();
    let mut fork2: Vec<Block> = Vec::new();

    let mut parent = genesis.clone();
    for _ in 0..5 {
        let new_block = gen_block(&parent, U256::from(40u64), vec![], vec![], vec![]);
        fork1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    let mut parent = genesis.clone();
    for _ in 0..2 {
        let new_block = gen_block(&parent, U256::from(80u64), vec![], vec![], vec![]);
        fork2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    // fork1 total_difficulty 200
    for blk in &fork1 {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    // fork2 total_difficulty 160
    for blk in &fork2 {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    let tip_number = { shared.lock_chain_state().tip_number() };

    // fork2 total_difficulty 260
    let new_block = gen_block(&parent, U256::from(100u64), vec![], vec![], vec![]);
    fork2.push(new_block.clone());

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: U256::zero(),
        total_uncles_count: 0,
        // if txs in parent is invalid, txs in block is also invalid
        txs_verified: None,
        dao_stats: DaoStats {
            accumulated_rate: DEFAULT_ACCUMULATED_RATE,
            accumulated_capacity: 0,
        },
    };

    let mut fork = ForkChanges::default();

    chain_service.find_fork(&mut fork, tip_number, &new_block, ext);

    let detached_blocks: HashSet<Block> = HashSet::from_iter(fork1.into_iter());
    let attached_blocks: HashSet<Block> = HashSet::from_iter(fork2.into_iter());
    assert_eq!(
        detached_blocks,
        HashSet::from_iter(fork.detached_blocks.iter().cloned())
    );
    assert_eq!(
        attached_blocks,
        HashSet::from_iter(fork.attached_blocks.iter().cloned())
    );
}
