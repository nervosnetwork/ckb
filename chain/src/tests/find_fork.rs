use crate::utils::forkchanges::ForkChanges;
use crate::verify::ConsumeUnverifiedBlockProcessor;
use crate::{UnverifiedBlock, start_chain_services};
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_proposal_table::ProposalTable;
use ckb_shared::SharedBuilder;
use ckb_store::ChainStore;
use ckb_systemtime::unix_time_as_millis;
use ckb_test_chain_utils::{MockChain, MockStore};
use ckb_types::{
    U256,
    core::{BlockBuilder, BlockExt, BlockView},
    packed,
    prelude::Pack,
};
use ckb_verification_traits::Switch;
use dashmap::DashSet;
use std::collections::HashSet;
use std::sync::Arc;

fn process_block(
    consume_unverified_block_processor: &mut ConsumeUnverifiedBlockProcessor,
    blk: &BlockView,
    switch: Switch,
) {
    let store = consume_unverified_block_processor.shared.store();
    let db_txn = store.begin_transaction();
    db_txn.insert_block(blk).unwrap();
    db_txn.commit().unwrap();

    let parent_header = store.get_block_header(&blk.parent_hash()).unwrap();
    let unverified_block = UnverifiedBlock {
        block: Arc::new(blk.to_owned()),
        switch: Some(switch),
        verify_callback: None,
        parent_header,
    };

    consume_unverified_block_processor.consume_unverified_blocks(unverified_block);
}

// 0--1--2--3--4
// \
//  \
//   1--2--3--4
#[test]
fn test_find_fork_case1() {
    let builder = SharedBuilder::with_temp_db();
    let consensus = Consensus::default();
    let proposal_table = ProposalTable::new(consensus.tx_proposal_window());
    let (shared, mut _pack) = builder.consensus(consensus).build().unwrap();

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let parent = genesis;
    let mock_store = MockStore::new(&parent, shared.store());
    let mut fork1 = MockChain::new(parent.clone(), shared.consensus());
    let mut fork2 = MockChain::new(parent, shared.consensus());
    for _ in 0..4 {
        fork1.gen_empty_block_with_diff(100u64, &mock_store);
    }

    for _ in 0..3 {
        fork2.gen_empty_block_with_diff(90u64, &mock_store);
    }

    let is_pending_verify = Arc::new(DashSet::new());

    let mut consume_unverified_block_processor = ConsumeUnverifiedBlockProcessor {
        shared: shared.clone(),
        is_pending_verify,
        proposal_table,
    };

    // fork1 total_difficulty 400
    for blk in fork1.blocks() {
        println!("proceb1, fork1 block: {}-{}", blk.number(), blk.hash());
        process_block(
            &mut consume_unverified_block_processor,
            blk,
            Switch::DISABLE_ALL,
        );
    }

    // fork2 total_difficulty 270
    for blk in fork2.blocks() {
        println!("procb2, fork1 block: {}-{}", blk.number(), blk.hash());
        process_block(
            &mut consume_unverified_block_processor,
            blk,
            Switch::DISABLE_ALL,
        );
    }

    let tip_number = { shared.snapshot().tip_number() };

    // fork2 total_difficulty 470
    fork2.gen_empty_block_with_diff(200u64, &mock_store);

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: U256::zero(),
        total_uncles_count: 0,
        // if txs in parent is invalid, txs in block is also invalid
        verified: None,
        txs_fees: vec![],
        cycles: None,
        txs_sizes: None,
    };

    let mut fork = ForkChanges::default();

    consume_unverified_block_processor.find_fork(&mut fork, tip_number, fork2.tip(), ext);

    let detached_blocks: HashSet<BlockView> = fork1.blocks().clone().into_iter().collect();
    let attached_blocks: HashSet<BlockView> = fork2.blocks().clone().into_iter().collect();
    assert_eq!(
        detached_blocks,
        fork.detached_blocks.iter().cloned().collect()
    );
    assert_eq!(
        attached_blocks,
        fork.attached_blocks.iter().cloned().collect()
    );
}

// 0--1--2--3--4
//    \
//     \
//      2--3--4
#[test]
fn test_find_fork_case2() {
    let builder = SharedBuilder::with_temp_db();
    let consensus = Consensus::default();
    let (shared, _pack) = builder.consensus(consensus.clone()).build().unwrap();

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();
    let mock_store = MockStore::new(&genesis, shared.store());
    let mut fork1 = MockChain::new(genesis, shared.consensus());

    for _ in 0..4 {
        fork1.gen_empty_block_with_diff(100u64, &mock_store);
    }

    let mut fork2 = MockChain::new(fork1.blocks()[0].header(), shared.consensus());
    for _ in 0..2 {
        fork2.gen_empty_block_with_diff(90u64, &mock_store);
    }
    let proposal_table = ProposalTable::new(consensus.tx_proposal_window());
    let mut consume_unverified_block_processor = ConsumeUnverifiedBlockProcessor {
        shared: shared.clone(),
        is_pending_verify: Arc::new(DashSet::new()),
        proposal_table,
    };

    // fork1 total_difficulty 400
    for blk in fork1.blocks() {
        process_block(
            &mut consume_unverified_block_processor,
            blk,
            Switch::DISABLE_ALL,
        );
    }

    // fork2 total_difficulty 280
    for blk in fork2.blocks() {
        process_block(
            &mut consume_unverified_block_processor,
            blk,
            Switch::DISABLE_ALL,
        );
    }

    let tip_number = { shared.snapshot().tip_number() };

    // fork2 total_difficulty 570
    fork2.gen_empty_block_with_inc_diff(200u64, &mock_store);

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: U256::zero(),
        total_uncles_count: 0,
        // if txs in parent is invalid, txs in block is also invalid
        verified: None,
        txs_fees: vec![],
        cycles: None,
        txs_sizes: None,
    };

    let mut fork = ForkChanges::default();

    consume_unverified_block_processor.find_fork(&mut fork, tip_number, fork2.tip(), ext);

    let detached_blocks: HashSet<BlockView> = fork1.blocks()[1..].iter().cloned().collect();
    let attached_blocks: HashSet<BlockView> = fork2.blocks().clone().into_iter().collect();
    assert_eq!(
        detached_blocks,
        fork.detached_blocks.iter().cloned().collect()
    );
    assert_eq!(
        attached_blocks,
        fork.attached_blocks.iter().cloned().collect()
    );
}

// 0--1--2--3
// \                _ fork
//  \             /
//   1--2--3--4--5--6
#[test]
fn test_find_fork_case3() {
    let builder = SharedBuilder::with_temp_db();
    let consensus = Consensus::default();
    let (shared, _pack) = builder.consensus(consensus.clone()).build().unwrap();

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let mock_store = MockStore::new(&genesis, shared.store());
    let mut fork1 = MockChain::new(genesis.clone(), shared.consensus());
    let mut fork2 = MockChain::new(genesis, shared.consensus());

    for _ in 0..3 {
        fork1.gen_empty_block_with_diff(80u64, &mock_store)
    }

    for _ in 0..5 {
        fork2.gen_empty_block_with_diff(40u64, &mock_store)
    }
    let proposal_table = ProposalTable::new(consensus.tx_proposal_window());
    let mut consume_unverified_block_processor = ConsumeUnverifiedBlockProcessor {
        shared: shared.clone(),
        is_pending_verify: Arc::new(DashSet::new()),
        proposal_table,
    };
    // fork1 total_difficulty 240
    for blk in fork1.blocks() {
        process_block(
            &mut consume_unverified_block_processor,
            blk,
            Switch::DISABLE_ALL,
        );
    }

    // fork2 total_difficulty 200
    for blk in fork2.blocks() {
        process_block(
            &mut consume_unverified_block_processor,
            blk,
            Switch::DISABLE_ALL,
        );
    }

    let tip_number = { shared.snapshot().tip_number() };

    // fork2 total_difficulty 300
    fork2.gen_empty_block_with_diff(100u64, &mock_store);

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: U256::zero(),
        total_uncles_count: 0,
        // if txs in parent is invalid, txs in block is also invalid
        verified: None,
        txs_fees: vec![],
        cycles: None,
        txs_sizes: None,
    };
    let mut fork = ForkChanges::default();

    consume_unverified_block_processor.find_fork(&mut fork, tip_number, fork2.tip(), ext);

    let detached_blocks: HashSet<BlockView> = fork1.blocks().clone().into_iter().collect();
    let attached_blocks: HashSet<BlockView> = fork2.blocks().clone().into_iter().collect();
    assert_eq!(
        detached_blocks,
        fork.detached_blocks.iter().cloned().collect()
    );
    assert_eq!(
        attached_blocks,
        fork.attached_blocks.iter().cloned().collect()
    );
}

// 0--1--2--3--4--5
// \        _ fork
//  \     /
//   1--2--3
#[test]
fn test_find_fork_case4() {
    let builder = SharedBuilder::with_temp_db();
    let consensus = Consensus::default();
    let (shared, _pack) = builder.consensus(consensus.clone()).build().unwrap();

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let mock_store = MockStore::new(&genesis, shared.store());
    let mut fork1 = MockChain::new(genesis.clone(), shared.consensus());
    let mut fork2 = MockChain::new(genesis, shared.consensus());

    for _ in 0..5 {
        fork1.gen_empty_block_with_diff(40u64, &mock_store);
    }

    for _ in 0..2 {
        fork2.gen_empty_block_with_diff(80u64, &mock_store);
    }
    let proposal_table = ProposalTable::new(consensus.tx_proposal_window());
    let mut consume_unverified_block_processor = ConsumeUnverifiedBlockProcessor {
        shared: shared.clone(),
        is_pending_verify: Arc::new(DashSet::new()),
        proposal_table,
    };

    // fork1 total_difficulty 200
    for blk in fork1.blocks() {
        process_block(
            &mut consume_unverified_block_processor,
            blk,
            Switch::DISABLE_ALL,
        );
    }

    // fork2 total_difficulty 160
    for blk in fork2.blocks() {
        process_block(
            &mut consume_unverified_block_processor,
            blk,
            Switch::DISABLE_ALL,
        );
    }

    let tip_number = { shared.snapshot().tip_number() };

    // fork2 total_difficulty 260
    fork2.gen_empty_block_with_diff(100u64, &mock_store);

    let ext = BlockExt {
        received_at: unix_time_as_millis(),
        total_difficulty: U256::zero(),
        total_uncles_count: 0,
        // if txs in parent is invalid, txs in block is also invalid
        verified: None,
        txs_fees: vec![],
        cycles: None,
        txs_sizes: None,
    };

    let mut fork = ForkChanges::default();

    consume_unverified_block_processor.find_fork(&mut fork, tip_number, fork2.tip(), ext);

    let detached_blocks: HashSet<BlockView> = fork1.blocks().clone().into_iter().collect();
    let attached_blocks: HashSet<BlockView> = fork2.blocks().clone().into_iter().collect();
    assert_eq!(
        detached_blocks,
        fork.detached_blocks.iter().cloned().collect()
    );
    assert_eq!(
        attached_blocks,
        fork.attached_blocks.iter().cloned().collect()
    );
}

// this case is create for issues from https://github.com/nervosnetwork/ckb/pull/1470
#[test]
fn repeatedly_switch_fork() {
    let consensus = Consensus::default();
    let (shared, mut pack) = SharedBuilder::with_temp_db()
        .consensus(consensus.clone())
        .build()
        .unwrap();
    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();
    let mock_store = MockStore::new(&genesis, shared.store());
    let mut fork1 = MockChain::new(genesis.clone(), shared.consensus());
    let mut fork2 = MockChain::new(genesis, shared.consensus());

    let chain_controller = start_chain_services(pack.take_chain_services_builder());

    for _ in 0..2 {
        fork1.gen_empty_block_with_nonce(1u128, &mock_store);
    }

    for _ in 0..2 {
        fork2.gen_empty_block_with_nonce(2u128, &mock_store);
    }
    let proposal_table = ProposalTable::new(consensus.tx_proposal_window());
    let mut consume_unverified_block_processor = ConsumeUnverifiedBlockProcessor {
        shared: shared.clone(),
        is_pending_verify: Arc::new(DashSet::new()),
        proposal_table,
    };

    for blk in fork1.blocks() {
        process_block(
            &mut consume_unverified_block_processor,
            blk,
            Switch::DISABLE_ALL,
        );
    }

    for blk in fork2.blocks() {
        process_block(
            &mut consume_unverified_block_processor,
            blk,
            Switch::DISABLE_ALL,
        );
    }

    //switch fork1
    let uncle = fork2.blocks().last().cloned().unwrap().as_uncle();
    let parent = fork1.blocks().last().cloned().unwrap();
    let epoch = shared
        .consensus()
        .next_epoch_ext(&parent.header(), &shared.store().borrow_as_data_loader())
        .unwrap()
        .epoch();
    let new_block1 = BlockBuilder::default()
        .parent_hash(parent.hash())
        .number((parent.number() + 1).pack())
        .compact_target(parent.compact_target().pack())
        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
        .nonce(1u128.pack())
        .uncle(uncle)
        .build();
    chain_controller
        .blocking_process_block_with_switch(Arc::new(new_block1.clone()), Switch::DISABLE_ALL)
        .unwrap();

    //switch fork2
    let mut parent = fork2.blocks().last().cloned().unwrap();
    let epoch = shared
        .consensus()
        .next_epoch_ext(&parent.header(), &shared.store().borrow_as_data_loader())
        .unwrap()
        .epoch();
    let new_block2 = BlockBuilder::default()
        .parent_hash(parent.hash())
        .number((parent.number() + 1).pack())
        .compact_target(parent.compact_target().pack())
        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
        .nonce(2u128.pack())
        .build();
    parent = new_block2.clone();
    chain_controller
        .blocking_process_block_with_switch(Arc::new(new_block2), Switch::DISABLE_ALL)
        .unwrap();
    let epoch = shared
        .consensus()
        .next_epoch_ext(&parent.header(), &shared.store().borrow_as_data_loader())
        .unwrap()
        .epoch();
    let new_block3 = BlockBuilder::default()
        .parent_hash(parent.hash())
        .number((parent.number() + 1).pack())
        .compact_target(parent.compact_target().pack())
        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
        .nonce(2u128.pack())
        .build();
    chain_controller
        .blocking_process_block_with_switch(Arc::new(new_block3), Switch::DISABLE_ALL)
        .unwrap();

    //switch fork1
    parent = new_block1;
    let epoch = shared
        .consensus()
        .next_epoch_ext(&parent.header(), &shared.store().borrow_as_data_loader())
        .unwrap()
        .epoch();
    let new_block4 = BlockBuilder::default()
        .parent_hash(parent.hash())
        .number((parent.number() + 1).pack())
        .compact_target(parent.compact_target().pack())
        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
        .nonce(1u128.pack())
        .build();
    chain_controller
        .blocking_process_block_with_switch(Arc::new(new_block4.clone()), Switch::DISABLE_ALL)
        .unwrap();

    parent = new_block4;
    let epoch = shared
        .consensus()
        .next_epoch_ext(&parent.header(), &shared.store().borrow_as_data_loader())
        .unwrap()
        .epoch();
    let new_block5 = BlockBuilder::default()
        .parent_hash(parent.hash())
        .number((parent.number() + 1).pack())
        .compact_target(parent.compact_target().pack())
        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
        .nonce(1u128.pack())
        .build();
    chain_controller
        .blocking_process_block_with_switch(Arc::new(new_block5), Switch::DISABLE_ALL)
        .unwrap();
}

// [ 1 <- 2 <- 3 ] <- 4 <- 5 <- 6 <- 7 <- 8 <- 9 <- 10 <- 11
//              \
//               \
//                - 4' <- 5'

#[test]
fn test_fork_proposal_table() {
    let builder = SharedBuilder::with_temp_db();
    let consensus = Consensus {
        tx_proposal_window: ProposalWindow(2, 3),
        ..Default::default()
    };

    let (shared, mut pack) = builder.consensus(consensus).build().unwrap();
    let chain_controller = start_chain_services(pack.take_chain_services_builder());

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let mock_store = MockStore::new(&genesis, shared.store());
    let mut mock = MockChain::new(genesis, shared.consensus());

    for i in 1..12 {
        let ids = vec![packed::ProposalShortId::new([
            0u8, 0, 0, 0, 0, 0, 0, 0, 0, i,
        ])];
        mock.gen_block_with_proposal_ids(40u64, ids, &mock_store);
    }

    for blk in mock.blocks() {
        chain_controller
            .blocking_process_block_with_switch(Arc::new(blk.clone()), Switch::DISABLE_ALL)
            .unwrap();
    }

    for _ in 1..9 {
        mock.rollback(&mock_store);
    }

    for i in 4..6 {
        let ids = vec![packed::ProposalShortId::new([
            1u8, 0, 0, 0, 0, 0, 0, 0, 0, i,
        ])];
        mock.gen_block_with_proposal_ids(200u64, ids, &mock_store);
    }

    for blk in mock.blocks().iter().skip(3) {
        chain_controller
            .blocking_process_block_with_switch(Arc::new(blk.clone()), Switch::DISABLE_ALL)
            .unwrap();
    }

    // snapshot proposals is prepare for tx-pool, validate on tip + 1
    let snapshot = shared.snapshot();
    let proposals = snapshot.proposals();

    assert_eq!(
        &vec![
            packed::ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 3]),
            packed::ProposalShortId::new([1u8, 0, 0, 0, 0, 0, 0, 0, 0, 4]),
        ]
        .into_iter()
        .collect::<HashSet<_>>(),
        proposals.set()
    );

    assert_eq!(
        &vec![packed::ProposalShortId::new([
            1u8, 0, 0, 0, 0, 0, 0, 0, 0, 5
        ])]
        .into_iter()
        .collect::<HashSet<_>>(),
        proposals.gap()
    );
}
