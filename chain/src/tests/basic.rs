use crate::tests::util::{
    create_transaction, create_transaction_with_out_point, gen_block, start_chain, MockChain,
};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::cell::{BlockInfo, CellMetaBuilder, CellProvider, CellStatus, UnresolvableError};
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutPoint, CellOutput, OutPoint, TransactionBuilder};
use ckb_core::{capacity_bytes, Bytes, Capacity};
use ckb_shared::error::SharedError;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use numext_fixed_uint::U256;
use std::sync::Arc;

#[test]
fn test_genesis_transaction_spend() {
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs(vec![
            CellOutput::new(
                capacity_bytes!(100_000_000),
                Bytes::default(),
                Script::default(),
                None
            );
            100
        ])
        .build();

    let mut root_hash = tx.hash().to_owned();

    let genesis_tx_hash = root_hash.clone();

    let genesis_block = BlockBuilder::default()
        .transaction(tx)
        .header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)))
        .build();

    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (chain_controller, shared, parent) = start_chain(Some(consensus));

    let end = 21;

    let mut chain = MockChain::new(parent.clone());

    for i in 1..end {
        let tx = create_transaction(&root_hash, i as u8);
        root_hash = tx.hash().to_owned();

        // commit txs in block
        chain.gen_block_with_commit_txs(vec![tx]);
    }

    for block in &chain.blocks()[0..10] {
        assert!(chain_controller
            .process_block(Arc::new(block.clone()), false)
            .is_ok());
    }

    assert_eq!(
        shared
            .lock_chain_state()
            .cell(&OutPoint::new_cell(genesis_tx_hash, 0)),
        CellStatus::Dead
    );
}

#[test]
fn test_transaction_spend_in_same_block() {
    let (chain_controller, shared, parent) = start_chain(None);
    let mut chain = MockChain::new(parent.clone());
    chain.gen_empty_block(100u64);

    let last_cell_base = &chain.tip().cellbase();
    let last_cell_base_hash = last_cell_base.hash().to_owned();
    let tx1 = create_transaction(&last_cell_base_hash, 1);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_transaction(&tx1_hash, 2);
    let tx2_hash = tx2.hash().to_owned();
    let tx2_output = tx2.outputs()[0].clone();

    let txs = vec![tx1, tx2];

    for hash in [&last_cell_base_hash, &tx1_hash, &tx2_hash].iter() {
        assert_eq!(
            shared
                .lock_chain_state()
                .cell(&OutPoint::new_cell(hash.to_owned().to_owned(), 0)),
            CellStatus::Unknown
        );
    }

    // proposal txs
    chain.gen_block_with_proposal_txs(txs.clone());
    // empty block
    chain.gen_empty_block(100);
    // commit txs in block
    chain.gen_block_with_commit_txs(txs.clone());

    for block in chain.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }

    for hash in [&last_cell_base_hash, &tx1_hash].iter() {
        assert_eq!(
            shared
                .lock_chain_state()
                .cell(&OutPoint::new_cell(hash.to_owned().to_owned(), 0)),
            CellStatus::Dead
        );
    }

    assert_eq!(
        shared
            .lock_chain_state()
            .cell(&OutPoint::new_cell(tx2_hash.to_owned(), 0)),
        CellStatus::live_cell(
            CellMetaBuilder::default()
                .out_point(CellOutPoint {
                    tx_hash: tx2_hash.to_owned(),
                    index: 0
                })
                .data_hash(tx2_output.data_hash())
                .capacity(tx2_output.capacity)
                .block_info(BlockInfo::new(4, 0))
                .build()
        )
    );
}

#[test]
fn test_transaction_conflict_in_same_block() {
    let (chain_controller, _shared, parent) = start_chain(None);
    let mut chain = MockChain::new(parent.clone());
    chain.gen_empty_block(100u64);

    let last_cell_base = &chain.tip().cellbase();
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_transaction(&tx1_hash, 2);
    let tx3 = create_transaction(&tx1_hash, 3);
    let txs = vec![tx1, tx2, tx3];

    // proposal txs
    chain.gen_block_with_proposal_txs(txs.clone());
    // empty block
    chain.gen_empty_block(100);
    // commit txs in block
    chain.gen_block_with_commit_txs(txs.clone());

    for block in chain.blocks().iter().take(3) {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }
    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Dead(OutPoint::new_cell(
            tx1_hash.to_owned(),
            0
        ))),
        chain_controller
            .process_block(Arc::new(chain.blocks()[3].clone()), true)
            .unwrap_err()
            .downcast()
            .unwrap()
    );
}

#[test]
fn test_transaction_conflict_in_different_blocks() {
    let (chain_controller, _shared, parent) = start_chain(None);
    let mut chain = MockChain::new(parent.clone());
    chain.gen_empty_block(100u64);

    let last_cell_base = &chain.tip().cellbase();
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(tx1_hash, 2);
    let tx3 = create_transaction(tx1_hash, 3);
    // proposal txs
    chain.gen_block_with_proposal_txs(vec![tx1.clone(), tx2.clone(), tx3.clone()]);

    // empty N+1 block
    chain.gen_empty_block(100);

    // commit tx1 and tx2 in N+2 block
    chain.gen_block_with_commit_txs(vec![tx1.clone(), tx2.clone()]);

    // commit tx3 in N+3 block
    chain.gen_block_with_commit_txs(vec![tx3.clone()]);

    for block in chain.blocks().iter().take(4) {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }
    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Dead(OutPoint::new_cell(
            tx1_hash.to_owned(),
            0
        ))),
        chain_controller
            .process_block(Arc::new(chain.blocks()[4].clone()), true)
            .unwrap_err()
            .downcast()
            .unwrap()
    );
}

#[test]
fn test_invalid_out_point_index_in_same_block() {
    let (chain_controller, _shared, parent) = start_chain(None);
    let mut chain = MockChain::new(parent.clone());
    chain.gen_empty_block(100u64);

    let last_cell_base = &chain.tip().cellbase();
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_transaction(&tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new_cell(tx1_hash.clone(), 1), 3);
    let txs = vec![tx1, tx2, tx3];
    // proposal txs
    chain.gen_block_with_proposal_txs(txs.clone());
    // empty N+1 block
    chain.gen_empty_block(100);
    // commit txs in N+2 block
    chain.gen_block_with_commit_txs(txs.clone());

    for block in chain.blocks().iter().take(3) {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }
    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Unknown(vec![OutPoint::new_cell(
            tx1_hash.to_owned(),
            1,
        )])),
        chain_controller
            .process_block(Arc::new(chain.blocks()[3].clone()), true)
            .unwrap_err()
            .downcast()
            .unwrap()
    );
}

#[test]
fn test_invalid_out_point_index_in_different_blocks() {
    let (chain_controller, _shared, parent) = start_chain(None);
    let mut chain = MockChain::new(parent.clone());
    chain.gen_empty_block(100u64);

    let last_cell_base = &chain.tip().cellbase();
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new_cell(tx1_hash.clone(), 1), 3);
    // proposal txs
    chain.gen_block_with_proposal_txs(vec![tx1.clone(), tx2.clone(), tx3.clone()]);
    // empty N+1 block
    chain.gen_empty_block(100);
    // commit tx1 and tx2 in N+2 block
    chain.gen_block_with_commit_txs(vec![tx1.clone(), tx2.clone()]);
    // commit tx3 in N+3 block
    chain.gen_block_with_commit_txs(vec![tx3.clone()]);

    for block in chain.blocks().iter().take(4) {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }

    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Unknown(vec![OutPoint::new_cell(
            tx1_hash.to_owned(),
            1,
        )])),
        chain_controller
            .process_block(Arc::new(chain.blocks()[4].clone()), true)
            .unwrap_err()
            .downcast()
            .unwrap()
    );
}

#[test]
fn test_genesis_transaction_fetch() {
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs(vec![
            CellOutput::new(
                capacity_bytes!(100_000_000),
                Bytes::default(),
                Script::default(),
                None
            );
            100
        ])
        .build();

    let root_hash = tx.hash().to_owned();

    let genesis_block = BlockBuilder::default()
        .transaction(tx)
        .header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)))
        .build();

    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (_chain_controller, shared, _parent) = start_chain(Some(consensus));

    let out_point = OutPoint::new_cell(root_hash, 0);
    let state = shared.lock_chain_state().cell(&out_point);
    assert!(state.is_live());
}

#[test]
fn test_chain_fork_by_total_difficulty() {
    let (chain_controller, shared, parent) = start_chain(None);
    let final_number = 20;

    let mut chain1 = MockChain::new(parent.clone());
    let mut chain2 = MockChain::new(parent.clone());

    for _ in 1..final_number {
        chain1.gen_empty_block(100u64);
    }

    for i in 1..final_number {
        let j = if i > 10 { 110 } else { 99 };
        chain2.gen_empty_block(j);
    }

    for block in chain1.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), false)
            .expect("process block ok");
    }

    for block in chain2.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), false)
            .expect("process block ok");
    }
    assert_eq!(
        shared.store().get_block_hash(8),
        chain2.blocks().get(7).map(|b| b.header().hash().to_owned())
    );
}

#[test]
fn test_chain_fork_by_hash() {
    let (chain_controller, shared, parent) = start_chain(None);
    let final_number = 20;

    let mut chain1 = MockChain::new(parent.clone());
    let mut chain2 = MockChain::new(parent.clone());

    for _ in 1..final_number {
        chain1.gen_empty_block(100u64);
    }

    for _ in 1..final_number {
        chain2.gen_empty_block(100u64);
    }

    for block in chain1.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), false)
            .expect("process block ok");
    }

    for block in chain2.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), false)
            .expect("process block ok");
    }

    //if total_difficulty equal, we chose block which have smaller hash as best
    assert!(chain1
        .blocks()
        .iter()
        .zip(chain2.blocks().iter())
        .all(|(a, b)| a.header().difficulty() == b.header().difficulty()));

    // TODO: chain1.hash is always equal to chain2.hash
    let best = if chain1.blocks()[(final_number - 2) as usize].header().hash()
        < chain2.blocks()[(final_number - 2) as usize].header().hash()
    {
        chain1
    } else {
        chain2
    };
    assert_eq!(
        shared.store().get_block_hash(8),
        best.blocks().get(7).map(|b| b.header().hash().to_owned())
    );
    assert_eq!(
        shared.store().get_block_hash(19),
        best.blocks().get(18).map(|b| b.header().hash().to_owned())
    );
}

#[test]
fn test_chain_get_ancestor() {
    let (chain_controller, shared, parent) = start_chain(None);
    let final_number = 20;

    let mut chain1 = MockChain::new(parent.clone());
    let mut chain2 = MockChain::new(parent.clone());

    for _ in 1..final_number {
        chain1.gen_empty_block(100u64);
    }

    for _ in 1..final_number {
        chain2.gen_empty_block(100u64);
    }

    for block in chain1.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), false)
            .expect("process block ok");
    }

    for block in chain2.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), false)
            .expect("process block ok");
    }

    assert_eq!(
        *chain1.blocks()[9].header(),
        shared
            .get_ancestor(&chain1.tip_header().hash(), 10)
            .unwrap()
    );

    // TODO: chain1 is always equals to chain2
    assert_eq!(
        *chain2.blocks()[9].header(),
        shared
            .get_ancestor(&chain2.tip_header().hash(), 10)
            .unwrap()
    );
}

#[test]
fn test_next_epoch_ext() {
    let genesis_block = BlockBuilder::default()
        .header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)))
        .build();
    let mut consensus = Consensus::default().set_genesis_block(genesis_block);
    consensus.genesis_epoch_ext.set_length(400);
    let epoch = consensus.genesis_epoch_ext.clone();

    let (chain_controller, shared, genesis) = start_chain(Some(consensus.clone()));
    let final_number = shared.consensus().genesis_epoch_ext().length();

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let mut parent = genesis.clone();
    let mut last_epoch = epoch.clone();

    for _ in 1..final_number - 1 {
        let epoch = shared
            .next_epoch_ext(&last_epoch, &parent)
            .unwrap_or(last_epoch);
        let new_block = gen_block(&parent, epoch.difficulty().clone(), vec![], vec![], vec![]);
        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain1.push(new_block.clone());
        parent = new_block.header().clone();
        last_epoch = epoch;
    }

    parent = genesis;
    let mut last_epoch = epoch.clone();
    for i in 1..final_number {
        let epoch = shared
            .next_epoch_ext(&last_epoch, &parent)
            .unwrap_or(last_epoch);
        let mut uncles = vec![];
        if i < 26 {
            uncles.push(chain1[i as usize].clone().into());
        }
        let new_block = gen_block(&parent, epoch.difficulty().clone(), vec![], vec![], uncles);
        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain2.push(new_block.clone());
        parent = new_block.header().clone();
        last_epoch = epoch;
    }
    {
        let chain_state = shared.lock_chain_state();
        let tip = chain_state.tip_header().clone();
        let total_uncles_count = shared
            .store()
            .get_block_ext(&tip.hash())
            .unwrap()
            .total_uncles_count;
        assert_eq!(total_uncles_count, 25);

        let epoch = shared
            .next_epoch_ext(chain_state.current_epoch_ext(), &tip)
            .unwrap();

        // last_uncles_count 25
        // last_epoch_length 400
        // epoch_duration_target 28800000
        // target_recip 20
        // last_duration 7980000

        // (Diff_last * o_last) / o
        // (25 * 1000 * 20) / 400
        assert_eq!(epoch.difficulty(), &U256::from(1250u64));

        // ((Cu_last + Cm_last) * L * Cm_last) / ((u + 1) * Cu_last * L_last)
        // ((25 + 400) * 28800000 * 400) / (( 20 + 1)* 25 * 7980000)
        assert_eq!(epoch.length(), 1168);

        let consensus = shared.consensus();

        let epoch_reward = consensus.epoch_reward();
        let block_reward = Capacity::shannons(epoch_reward.as_u64() / epoch.length());
        let block_reward1 = block_reward.safe_add(Capacity::one()).unwrap();
        let bound = 400 + epoch.remainder_reward().as_u64();

        // block_reward 428082191780
        // remainder_reward 960
        assert_eq!(
            epoch.block_reward(400).unwrap(),
            block_reward1, // Capacity::shannons(428082191781)
            "block_reward {:?}, remainder_reward{:?}",
            block_reward,
            epoch.remainder_reward()
        );

        assert_eq!(
            epoch.block_reward(bound - 1).unwrap(),
            block_reward1 // Capacity::shannons(428082191781)
        );
        assert_eq!(
            epoch.block_reward(bound).unwrap(),
            block_reward // Capacity::shannons(428082191780)
        );
    }

    let (chain_controller, shared, genesis) = start_chain(Some(consensus.clone()));
    let mut chain2: Vec<Block> = Vec::new();
    for i in 1..final_number - 1 {
        chain_controller
            .process_block(Arc::new(chain1[(i - 1) as usize].clone()), false)
            .expect("process block ok");
    }

    parent = genesis.clone();
    for i in 1..final_number {
        let epoch = shared
            .next_epoch_ext(&last_epoch, &parent)
            .unwrap_or(last_epoch);
        let mut uncles = vec![];
        if i < 11 {
            uncles.push(chain1[i as usize].clone().into());
        }
        let new_block = gen_block(&parent, epoch.difficulty().clone(), vec![], vec![], uncles);
        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain2.push(new_block.clone());
        parent = new_block.header().clone();
        last_epoch = epoch;
    }

    {
        let chain_state = shared.lock_chain_state();
        let tip = chain_state.tip_header().clone();
        let total_uncles_count = shared
            .store()
            .get_block_ext(&tip.hash())
            .unwrap()
            .total_uncles_count;
        assert_eq!(total_uncles_count, 10);

        let epoch = shared
            .next_epoch_ext(chain_state.current_epoch_ext(), &tip)
            .unwrap();

        assert_eq!(epoch.difficulty(), &U256::from(1000u64));
    }

    let (chain_controller, shared, genesis) = start_chain(Some(consensus.clone()));
    let mut chain2: Vec<Block> = Vec::new();
    for i in 1..final_number - 1 {
        chain_controller
            .process_block(Arc::new(chain1[(i - 1) as usize].clone()), false)
            .expect("process block ok");
    }

    parent = genesis.clone();
    let mut last_epoch = epoch.clone();
    for i in 1..final_number {
        let epoch = shared
            .next_epoch_ext(&last_epoch, &parent)
            .unwrap_or(last_epoch);
        let mut uncles = vec![];
        if i < 151 {
            uncles.push(chain1[i as usize].clone().into());
        }
        let new_block = gen_block(&parent, epoch.difficulty().clone(), vec![], vec![], uncles);
        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain2.push(new_block.clone());
        parent = new_block.header().clone();
        last_epoch = epoch;
    }

    {
        let chain_state = shared.lock_chain_state();
        let tip = chain_state.tip_header().clone();
        let total_uncles_count = shared
            .store()
            .get_block_ext(&tip.hash())
            .unwrap()
            .total_uncles_count;
        assert_eq!(total_uncles_count, 150);

        let epoch = shared
            .next_epoch_ext(chain_state.current_epoch_ext(), &tip)
            .unwrap();
        // max[150 * 10 * 1000 / 200, 2 * 1000]
        assert_eq!(epoch.difficulty(), &U256::from(2000u64));
    }
}
