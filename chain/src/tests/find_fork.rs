use crate::chain::{ChainService, ForkChanges};
use crate::tests::util::{MockChain, MockStore};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::extras::BlockExt;
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::NotifyService;
use ckb_shared::shared::SharedBuilder;
use ckb_store::ChainStore;
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
    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let parent = genesis.clone();
    let mut mock_store = MockStore::new(&parent, shared.store());
    let mut fork1 = MockChain::new(parent.clone(), shared.consensus());
    let mut fork2 = MockChain::new(parent.clone(), shared.consensus());
    for _ in 0..4 {
        fork1.gen_empty_block_with_difficulty(100u64, &mut mock_store);
    }

    for _ in 0..3 {
        fork2.gen_empty_block_with_difficulty(90u64, &mut mock_store);
    }

    // fork1 total_difficulty 400
    for blk in fork1.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    // fork2 total_difficulty 270
    for blk in fork2.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    let tip_number = { shared.lock_chain_state().tip_number() };

    // fork2 total_difficulty 470
    fork2.gen_empty_block_with_difficulty(200u64, &mut mock_store);

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: U256::zero(),
        total_uncles_count: 0,
        // if txs in parent is invalid, txs in block is also invalid
        verified: None,
        txs_fees: vec![],
    };

    let mut fork = ForkChanges::default();

    chain_service.find_fork(&mut fork, tip_number, fork2.tip(), ext);

    let detached_blocks: HashSet<Block> = HashSet::from_iter(fork1.blocks().clone().into_iter());
    let attached_blocks: HashSet<Block> = HashSet::from_iter(fork2.blocks().clone().into_iter());
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

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();
    let mut mock_store = MockStore::new(&genesis, shared.store());
    let mut fork1 = MockChain::new(genesis.clone(), shared.consensus());

    for _ in 0..4 {
        fork1.gen_empty_block_with_difficulty(100u64, &mut mock_store);
    }

    let mut fork2 = MockChain::new(fork1.blocks()[0].header().to_owned(), shared.consensus());
    for _ in 0..2 {
        fork2.gen_empty_block_with_difficulty(90u64, &mut mock_store);
    }

    // fork1 total_difficulty 400
    for blk in fork1.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    // fork2 total_difficulty 280
    for blk in fork2.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    let tip_number = { shared.lock_chain_state().tip_number() };

    // fork2 total_difficulty 570
    fork2.gen_empty_block(200u64, &mut mock_store);

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: U256::zero(),
        total_uncles_count: 0,
        // if txs in parent is invalid, txs in block is also invalid
        verified: None,
        txs_fees: vec![],
    };

    let mut fork = ForkChanges::default();

    chain_service.find_fork(&mut fork, tip_number, fork2.tip(), ext);

    let detached_blocks: HashSet<Block> = HashSet::from_iter(fork1.blocks()[1..].iter().cloned());
    let attached_blocks: HashSet<Block> = HashSet::from_iter(fork2.blocks().clone().into_iter());
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

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let mut mock_store = MockStore::new(&genesis, shared.store());
    let mut fork1 = MockChain::new(genesis.clone(), shared.consensus());
    let mut fork2 = MockChain::new(genesis.clone(), shared.consensus());

    for _ in 0..3 {
        fork1.gen_empty_block_with_difficulty(80u64, &mut mock_store)
    }

    for _ in 0..5 {
        fork2.gen_empty_block_with_difficulty(40u64, &mut mock_store)
    }

    // fork1 total_difficulty 240
    for blk in fork1.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    // fork2 total_difficulty 200
    for blk in fork2.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    let tip_number = { shared.lock_chain_state().tip_number() };

    // fork2 total_difficulty 300
    fork2.gen_empty_block_with_difficulty(100u64, &mut mock_store);

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: U256::zero(),
        total_uncles_count: 0,
        // if txs in parent is invalid, txs in block is also invalid
        verified: None,
        txs_fees: vec![],
    };
    let mut fork = ForkChanges::default();

    chain_service.find_fork(&mut fork, tip_number, fork2.tip(), ext);

    let detached_blocks: HashSet<Block> = HashSet::from_iter(fork1.blocks().clone().into_iter());
    let attached_blocks: HashSet<Block> = HashSet::from_iter(fork2.blocks().clone().into_iter());
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

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let mut mock_store = MockStore::new(&genesis, shared.store());
    let mut fork1 = MockChain::new(genesis.clone(), shared.consensus());
    let mut fork2 = MockChain::new(genesis.clone(), shared.consensus());

    for _ in 0..5 {
        fork1.gen_empty_block_with_difficulty(40u64, &mut mock_store);
    }

    for _ in 0..2 {
        fork2.gen_empty_block_with_difficulty(80u64, &mut mock_store);
    }

    // fork1 total_difficulty 200
    for blk in fork1.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    // fork2 total_difficulty 160
    for blk in fork2.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    let tip_number = { shared.lock_chain_state().tip_number() };

    // fork2 total_difficulty 260
    fork2.gen_empty_block_with_difficulty(100u64, &mut mock_store);

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: U256::zero(),
        total_uncles_count: 0,
        // if txs in parent is invalid, txs in block is also invalid
        verified: None,
        txs_fees: vec![],
    };

    let mut fork = ForkChanges::default();

    chain_service.find_fork(&mut fork, tip_number, fork2.tip(), ext);

    let detached_blocks: HashSet<Block> = HashSet::from_iter(fork1.blocks().clone().into_iter());
    let attached_blocks: HashSet<Block> = HashSet::from_iter(fork2.blocks().clone().into_iter());
    assert_eq!(
        detached_blocks,
        HashSet::from_iter(fork.detached_blocks.iter().cloned())
    );
    assert_eq!(
        attached_blocks,
        HashSet::from_iter(fork.attached_blocks.iter().cloned())
    );
}
