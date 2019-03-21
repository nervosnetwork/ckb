use crate::tests::util::{create_transaction, gen_block, start_chain};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::block::BlockBuilder;
use ckb_core::cell::CellProvider;
use ckb_core::header::HeaderBuilder;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
use ckb_traits::ChainProvider;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::sync::Arc;

#[test]
fn test_genesis_transaction_spend() {
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), Default::default()))
        .outputs(vec![
            CellOutput::new(
                100_000_000,
                vec![],
                H256::default(),
                None
            );
            100
        ])
        .build();

    let mut root_hash = tx.hash().clone();

    let genesis_block = BlockBuilder::default()
        .commit_transaction(tx)
        .with_header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)));

    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (chain_controller, shared) = start_chain(Some(consensus), false);

    let end = 21;

    let mut blocks1: Vec<Block> = vec![];
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..end {
        let difficulty = parent.difficulty().clone();
        let tx = create_transaction(root_hash);
        root_hash = tx.hash().clone();
        let new_block = gen_block(&parent, difficulty + U256::from(1u64), vec![tx], vec![], vec![]);
        blocks1.push(new_block.clone());
        parent = new_block.header().clone();
    }

    for block in &blocks1[0..10] {
        assert!(chain_controller
            .process_block(Arc::new(block.clone()))
            .is_ok());
    }
}

#[test]
fn test_genesis_transaction_fetch() {
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), Default::default()))
        .outputs(vec![
            CellOutput::new(
                100_000_000,
                vec![],
                H256::default(),
                None
            );
            100
        ])
        .build();

    let root_hash = tx.hash().clone();

    let genesis_block = BlockBuilder::default()
        .commit_transaction(tx)
        .with_header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)));

    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (_chain_controller, shared) = start_chain(Some(consensus), false);

    let out_point = OutPoint::new(root_hash, 0);
    let state = shared.chain_state().lock().cell(&out_point);
    assert!(state.is_live());
}

#[test]
fn test_chain_fork_by_total_difficulty() {
    let (chain_controller, shared) = start_chain(None, false);
    let final_number = 20;

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(&parent, difficulty + U256::from(100u64), vec![], vec![], vec![]);
        chain1.push(new_block.clone());
        parent = new_block.header().clone();
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for i in 1..final_number {
        let difficulty = parent.difficulty().clone();
        let j = if i > 10 { 110 } else { 99 };
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(j as u32),
            vec![],
            vec![],
            vec![],
        );
        chain2.push(new_block.clone());
        parent = new_block.header().clone();
    }

    for block in &chain1 {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }

    for block in &chain2 {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }
    assert_eq!(
        shared.block_hash(8),
        chain2.get(7).map(|b| b.header().hash())
    );
}

#[test]
fn test_chain_fork_by_hash() {
    let (chain_controller, shared) = start_chain(None, false);
    let final_number = 20;

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(&parent, difficulty + U256::from(100u64), vec![], vec![], vec![]);
        chain1.push(new_block.clone());
        parent = new_block.header().clone();
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        chain2.push(new_block.clone());
        parent = new_block.header().clone();
    }

    for block in &chain1 {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }

    for block in &chain2 {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }

    //if total_difficulty equal, we chose block which have smaller hash as best
    assert!(chain1
        .iter()
        .zip(chain2.iter())
        .all(|(a, b)| a.header().difficulty() == b.header().difficulty()));

    let best = if chain1[(final_number - 2) as usize].header().hash()
        < chain2[(final_number - 2) as usize].header().hash()
    {
        chain1
    } else {
        chain2
    };
    assert_eq!(shared.block_hash(8), best.get(7).map(|b| b.header().hash()));
    assert_eq!(
        shared.block_hash(19),
        best.get(18).map(|b| b.header().hash())
    );
}

#[test]
fn test_chain_get_ancestor() {
    let (chain_controller, shared) = start_chain(None, false);
    let final_number = 20;

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(&parent, difficulty + U256::from(100u64), vec![], vec![], vec![]);
        chain1.push(new_block.clone());
        parent = new_block.header().clone();
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        chain2.push(new_block.clone());
        parent = new_block.header().clone();
    }

    for block in &chain1 {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }

    for block in &chain2 {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }

    assert_eq!(
        *chain1[9].header(),
        shared
            .get_ancestor(&chain1.last().unwrap().header().hash(), 10)
            .unwrap()
    );

    assert_eq!(
        *chain2[9].header(),
        shared
            .get_ancestor(&chain2.last().unwrap().header().hash(), 10)
            .unwrap()
    );
}

#[test]
fn test_calculate_difficulty() {
    let genesis_block = BlockBuilder::default()
        .with_header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)));
    let mut consensus = Consensus::default().set_genesis_block(genesis_block);
    consensus.pow_time_span = 200;
    consensus.pow_spacing = 1;

    let (chain_controller, shared) = start_chain(Some(consensus.clone()), false);
    let final_number = shared.consensus().difficulty_adjustment_interval();

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number - 1 {
        let difficulty = shared.calculate_difficulty(&parent).unwrap();
        let new_block = gen_block(&parent, difficulty, vec![], vec![], vec![]);
        chain_controller
            .process_block(Arc::new(new_block.clone()))
            .expect("process block ok");
        chain1.push(new_block.clone());
        parent = new_block.header().clone();
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for i in 1..final_number {
        let difficulty = shared.calculate_difficulty(&parent).unwrap();
        let mut uncles = vec![];
        if i < 26 {
            uncles.push(chain1[i as usize].clone().into());
        }
        let new_block = gen_block(&parent, difficulty, vec![], vec![], uncles);
        chain_controller
            .process_block(Arc::new(new_block.clone()))
            .expect("process block ok");
        chain2.push(new_block.clone());
        parent = new_block.header().clone();
    }
    let tip = shared.chain_state().lock().tip_header().clone();
    let total_uncles_count = shared.block_ext(&tip.hash()).unwrap().total_uncles_count;
    assert_eq!(total_uncles_count, 25);
    let difficulty = shared.calculate_difficulty(&tip).unwrap();

    // 25 * 10 * 1000 / 200
    assert_eq!(difficulty, U256::from(1250u64));

    let (chain_controller, shared) = start_chain(Some(consensus.clone()), false);
    let mut chain2: Vec<Block> = Vec::new();
    for i in 1..final_number - 1 {
        chain_controller
            .process_block(Arc::new(chain1[(i - 1) as usize].clone()))
            .expect("process block ok");
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for i in 1..final_number {
        let difficulty = shared.calculate_difficulty(&parent).unwrap();
        let mut uncles = vec![];
        if i < 11 {
            uncles.push(chain1[i as usize].clone().into());
        }
        let new_block = gen_block(&parent, difficulty, vec![], vec![], uncles);
        chain_controller
            .process_block(Arc::new(new_block.clone()))
            .expect("process block ok");
        chain2.push(new_block.clone());
        parent = new_block.header().clone();
    }
    let tip = shared.chain_state().lock().tip_header().clone();
    let total_uncles_count = shared.block_ext(&tip.hash()).unwrap().total_uncles_count;
    assert_eq!(total_uncles_count, 10);
    let difficulty = shared.calculate_difficulty(&tip).unwrap();

    // min[10 * 10 * 1000 / 200, 1000]
    assert_eq!(difficulty, U256::from(1000u64));

    let (chain_controller, shared) = start_chain(Some(consensus.clone()), false);
    let mut chain2: Vec<Block> = Vec::new();
    for i in 1..final_number - 1 {
        chain_controller
            .process_block(Arc::new(chain1[(i - 1) as usize].clone()))
            .expect("process block ok");
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for i in 1..final_number {
        let difficulty = shared.calculate_difficulty(&parent).unwrap();
        let mut uncles = vec![];
        if i < 151 {
            uncles.push(chain1[i as usize].clone().into());
        }
        let new_block = gen_block(&parent, difficulty, vec![], vec![], uncles);
        chain_controller
            .process_block(Arc::new(new_block.clone()))
            .expect("process block ok");
        chain2.push(new_block.clone());
        parent = new_block.header().clone();
    }
    let tip = shared.chain_state().lock().tip_header().clone();
    let total_uncles_count = shared.block_ext(&tip.hash()).unwrap().total_uncles_count;
    assert_eq!(total_uncles_count, 150);
    let difficulty = shared.calculate_difficulty(&tip).unwrap();

    // max[150 * 10 * 1000 / 200, 2 * 1000]
    assert_eq!(difficulty, U256::from(2000u64));
}
