use crate::chain::{ChainService, ForkChanges};
use crate::tests::util::{MockChain, MockStore};
use ckb_chain_spec::consensus::Consensus;
use ckb_notify::NotifyService;
use ckb_shared::shared::SharedBuilder;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use ckb_types::{
    core::{BlockBuilder, BlockExt, BlockView},
    prelude::Pack,
    U256,
};
use faketime::unix_time_as_millis;
use std::collections::HashSet;
use std::iter::FromIterator;
use std::sync::Arc;

// 0--1--2--3--4
// \
//  \
//   1--2--3--4
#[test]
fn test_find_fork_case1() {
    let builder = SharedBuilder::default();
    let (shared, table) = builder.consensus(Consensus::default()).build().unwrap();
    let mut chain_service = ChainService::new(shared.clone(), table);
    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let parent = genesis.clone();
    let mock_store = MockStore::new(&parent, shared.store());
    let mut fork1 = MockChain::new(parent.clone(), shared.consensus());
    let mut fork2 = MockChain::new(parent.clone(), shared.consensus());
    for _ in 0..4 {
        fork1.gen_empty_block_with_difficulty(100u64, &mock_store);
    }

    for _ in 0..3 {
        fork2.gen_empty_block_with_difficulty(90u64, &mock_store);
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

    let tip_number = { shared.snapshot().tip_number() };

    // fork2 total_difficulty 470
    fork2.gen_empty_block_with_difficulty(200u64, &mock_store);

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

    let detached_blocks: HashSet<BlockView> =
        HashSet::from_iter(fork1.blocks().clone().into_iter());
    let attached_blocks: HashSet<BlockView> =
        HashSet::from_iter(fork2.blocks().clone().into_iter());
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
    let builder = SharedBuilder::default();
    let (shared, table) = builder.consensus(Consensus::default()).build().unwrap();
    let mut chain_service = ChainService::new(shared.clone(), table);

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();
    let mock_store = MockStore::new(&genesis, shared.store());
    let mut fork1 = MockChain::new(genesis.clone(), shared.consensus());

    for _ in 0..4 {
        fork1.gen_empty_block_with_difficulty(100u64, &mock_store);
    }

    let mut fork2 = MockChain::new(fork1.blocks()[0].header().to_owned(), shared.consensus());
    for _ in 0..2 {
        fork2.gen_empty_block_with_difficulty(90u64, &mock_store);
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

    let tip_number = { shared.snapshot().tip_number() };

    // fork2 total_difficulty 570
    fork2.gen_empty_block(200u64, &mock_store);

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

    let detached_blocks: HashSet<BlockView> =
        HashSet::from_iter(fork1.blocks()[1..].iter().cloned());
    let attached_blocks: HashSet<BlockView> =
        HashSet::from_iter(fork2.blocks().clone().into_iter());
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
    let builder = SharedBuilder::default();
    let (shared, table) = builder.consensus(Consensus::default()).build().unwrap();
    let mut chain_service = ChainService::new(shared.clone(), table);

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let mock_store = MockStore::new(&genesis, shared.store());
    let mut fork1 = MockChain::new(genesis.clone(), shared.consensus());
    let mut fork2 = MockChain::new(genesis.clone(), shared.consensus());

    for _ in 0..3 {
        fork1.gen_empty_block_with_difficulty(80u64, &mock_store)
    }

    for _ in 0..5 {
        fork2.gen_empty_block_with_difficulty(40u64, &mock_store)
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

    let tip_number = { shared.snapshot().tip_number() };

    // fork2 total_difficulty 300
    fork2.gen_empty_block_with_difficulty(100u64, &mock_store);

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

    let detached_blocks: HashSet<BlockView> =
        HashSet::from_iter(fork1.blocks().clone().into_iter());
    let attached_blocks: HashSet<BlockView> =
        HashSet::from_iter(fork2.blocks().clone().into_iter());
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
    let builder = SharedBuilder::default();
    let (shared, table) = builder.consensus(Consensus::default()).build().unwrap();
    let mut chain_service = ChainService::new(shared.clone(), table);

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let mock_store = MockStore::new(&genesis, shared.store());
    let mut fork1 = MockChain::new(genesis.clone(), shared.consensus());
    let mut fork2 = MockChain::new(genesis.clone(), shared.consensus());

    for _ in 0..5 {
        fork1.gen_empty_block_with_difficulty(40u64, &mock_store);
    }

    for _ in 0..2 {
        fork2.gen_empty_block_with_difficulty(80u64, &mock_store);
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

    let tip_number = { shared.snapshot().tip_number() };

    // fork2 total_difficulty 260
    fork2.gen_empty_block_with_difficulty(100u64, &mock_store);

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

    let detached_blocks: HashSet<BlockView> =
        HashSet::from_iter(fork1.blocks().clone().into_iter());
    let attached_blocks: HashSet<BlockView> =
        HashSet::from_iter(fork2.blocks().clone().into_iter());
    assert_eq!(
        detached_blocks,
        HashSet::from_iter(fork.detached_blocks.iter().cloned())
    );
    assert_eq!(
        attached_blocks,
        HashSet::from_iter(fork.attached_blocks.iter().cloned())
    );
}

// this case is create for issuse from https://github.com/nervosnetwork/ckb/pull/1470
#[test]
fn repeatedly_switch_fork() {
    ckb_store::set_cache_enable(false);
    let (shared, _) = SharedBuilder::default()
        .consensus(Consensus::default())
        .build()
        .unwrap();
    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();
    let mock_store = MockStore::new(&genesis, shared.store());
    let mut fork1 = MockChain::new(genesis.clone(), shared.consensus());
    let mut fork2 = MockChain::new(genesis.clone(), shared.consensus());

    let notify = NotifyService::default().start::<&str>(None);
    let (shared, table) = SharedBuilder::default()
        .consensus(Consensus::default())
        .build()
        .unwrap();
    let mut chain_service = ChainService::new(shared, table);

    for _ in 0..2 {
        fork1.gen_empty_block_with_nonce(1u64, &mock_store);
    }

    for _ in 0..2 {
        fork2.gen_empty_block_with_nonce(2u64, &mock_store);
    }

    for blk in fork1.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    for blk in fork2.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), false)
            .unwrap();
    }

    //switch fork1
    let uncle = fork2.blocks().last().cloned().unwrap().as_uncle();
    let parent = fork1.blocks().last().cloned().unwrap();
    let new_block1 = BlockBuilder::default()
        .parent_hash(parent.hash().to_owned())
        .number((parent.number() + 1).pack())
        .difficulty(parent.difficulty().pack())
        .nonce(1u64.pack())
        .uncle(uncle)
        .build();
    chain_service
        .process_block(Arc::new(new_block1.clone()), false)
        .unwrap();

    //switch fork2
    let mut parent = fork2.blocks().last().cloned().unwrap();
    let new_block2 = BlockBuilder::default()
        .parent_hash(parent.hash().to_owned())
        .number((parent.number() + 1).pack())
        .difficulty(parent.difficulty().pack())
        .nonce(2u64.pack())
        .build();
    parent = new_block2.clone();
    chain_service
        .process_block(Arc::new(new_block2), false)
        .unwrap();
    let new_block3 = BlockBuilder::default()
        .parent_hash(parent.hash().to_owned())
        .number((parent.number() + 1).pack())
        .difficulty(parent.difficulty().pack())
        .nonce(2u64.pack())
        .build();
    chain_service
        .process_block(Arc::new(new_block3), false)
        .unwrap();

    //switch fork1
    parent = new_block1.clone();
    let new_block4 = BlockBuilder::default()
        .parent_hash(parent.hash().to_owned())
        .number((parent.number() + 1).pack())
        .difficulty(parent.difficulty().pack())
        .nonce(1u64.pack())
        .build();
    chain_service
        .process_block(Arc::new(new_block4.clone()), false)
        .unwrap();

    parent = new_block4.clone();
    let new_block5 = BlockBuilder::default()
        .parent_hash(parent.hash().to_owned())
        .number((parent.number() + 1).pack())
        .difficulty(parent.difficulty().pack())
        .nonce(1u64.pack())
        .build();
    chain_service
        .process_block(Arc::new(new_block5), false)
        .unwrap();
}
