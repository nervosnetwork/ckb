use crate::tests::util::{create_transaction, gen_block, start_chain};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::block::BlockBuilder;
use ckb_core::cell::{CellMeta, CellProvider, CellStatus};
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
use ckb_core::{capacity_bytes, Bytes, Capacity};
use ckb_shared::error::SharedError;
use ckb_traits::ChainProvider;
use numext_fixed_uint::U256;
use std::sync::Arc;

#[test]
fn test_genesis_transaction_spend() {
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0, Default::default()))
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

    let mut root_hash = tx.hash().clone();

    let genesis_tx_hash = root_hash.clone();

    let genesis_block = BlockBuilder::default()
        .transaction(tx)
        .with_header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)));

    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (chain_controller, shared) = start_chain(Some(consensus), false);

    let end = 21;

    let mut blocks1: Vec<Block> = vec![];
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for i in 1..end {
        let difficulty = parent.difficulty().clone();
        let tx = create_transaction(root_hash, i as u8);
        root_hash = tx.hash().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(1u64),
            vec![tx],
            vec![],
            vec![],
        );
        blocks1.push(new_block.clone());
        parent = new_block.header().clone();
    }

    for block in &blocks1[0..10] {
        assert!(chain_controller
            .process_block(Arc::new(block.clone()))
            .is_ok());
    }

    assert_eq!(
        shared
            .chain_state()
            .lock()
            .get_cell_status(&OutPoint::new(genesis_tx_hash, 0)),
        CellStatus::Dead
    );
}

#[test]
fn test_transaction_spend_in_same_block() {
    let (chain_controller, shared) = start_chain(None, true);
    let mut chain: Vec<Block> = Vec::new();
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().clone();
        chain.push(new_block);
    }

    let last_cell_base = &chain.last().unwrap().transactions()[0];
    let last_cell_base_hash = last_cell_base.hash().clone();
    let tx1 = create_transaction(last_cell_base_hash.clone(), 1);
    let tx1_hash = tx1.hash().clone();
    let tx2 = create_transaction(tx1_hash.clone(), 2);
    let tx2_hash = tx2.hash().clone();
    let tx2_output = tx2.outputs()[0].clone();

    let txs = vec![tx1, tx2];

    for hash in [
        last_cell_base_hash.clone(),
        tx1_hash.clone(),
        tx2_hash.clone(),
    ]
    .iter()
    {
        assert_eq!(
            shared
                .chain_state()
                .lock()
                .get_cell_status(&OutPoint::new(hash.clone(), 0)),
            CellStatus::Unknown
        );
    }
    // proposal txs
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            txs.clone(),
            vec![],
        );
        parent = new_block.header().clone();
        chain.push(new_block);
    }
    // empty N+1 block
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().clone();
        chain.push(new_block);
    }
    // commit txs in N+2 block
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            txs.clone(),
            vec![],
            vec![],
        );
        chain.push(new_block);
    }
    for block in &chain {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }

    for hash in [last_cell_base_hash, tx1_hash].iter() {
        assert_eq!(
            shared
                .chain_state()
                .lock()
                .get_cell_status(&OutPoint::new(hash.clone(), 0)),
            CellStatus::Dead
        );
    }

    assert_eq!(
        shared
            .chain_state()
            .lock()
            .get_cell_status(&OutPoint::new(tx2_hash.clone(), 0)),
        CellStatus::live_cell(CellMeta {
            cell_output: None,
            out_point: OutPoint {
                tx_hash: tx2_hash,
                index: 0
            },
            cellbase: false,
            capacity: tx2_output.capacity,
            data_hash: Some(tx2_output.data_hash()),
            block_number: Some(4),
        })
    );
}

#[test]
fn test_transaction_conflict_in_same_block() {
    let (chain_controller, shared) = start_chain(None, true);
    let mut chain: Vec<Block> = Vec::new();
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().clone();
        chain.push(new_block);
    }

    let last_cell_base = &chain.last().unwrap().transactions()[0];
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx2 = create_transaction(tx1.hash(), 2);
    let tx3 = create_transaction(tx1.hash(), 3);
    let txs = vec![tx1, tx2, tx3];
    // proposal txs
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            txs.clone(),
            vec![],
        );
        parent = new_block.header().clone();
        chain.push(new_block);
    }
    // empty N+1 block
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().clone();
        chain.push(new_block);
    }
    // commit txs in N+2 block
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            txs.clone(),
            vec![],
            vec![],
        );
        chain.push(new_block);
    }
    for block in chain.iter().take(3) {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }
    let error = chain_controller
        .process_block(Arc::new(chain[3].clone()))
        .unwrap_err()
        .downcast()
        .unwrap();
    if let SharedError::InvalidTransaction(errmsg) = error {
        let re = regex::Regex::new(r#"Transactions\(\([0-9], Conflict\)\)"#).unwrap();
        assert!(re.is_match(&errmsg));
    } else {
        panic!("should be the Conflict Transactions error");
    }
}

#[test]
fn test_transaction_conflict_in_different_blocks() {
    let (chain_controller, shared) = start_chain(None, true);
    let mut chain: Vec<Block> = Vec::new();
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().clone();
        chain.push(new_block);
    }

    let last_cell_base = &chain.last().unwrap().transactions()[0];
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx2 = create_transaction(tx1.hash(), 2);
    let tx3 = create_transaction(tx1.hash(), 3);
    // proposal txs
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![tx1.clone(), tx2.clone(), tx3.clone()],
            vec![],
        );
        parent = new_block.header().clone();
        chain.push(new_block);
    }
    // empty N+1 block
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().clone();
        chain.push(new_block);
    }
    // commit tx1 and tx2 in N+2 block
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![tx1.clone(), tx2.clone()],
            vec![],
            vec![],
        );
        parent = new_block.header().clone();
        chain.push(new_block);
    }
    // commit tx3 in N+3 block
    {
        let difficulty = parent.difficulty().clone();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![tx3.clone()],
            vec![],
            vec![],
        );
        chain.push(new_block);
    }
    for block in chain.iter().take(4) {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }
    let error = chain_controller
        .process_block(Arc::new(chain[4].clone()))
        .unwrap_err()
        .downcast()
        .unwrap();
    if let SharedError::InvalidTransaction(errmsg) = error {
        let re = regex::Regex::new(r#"Transactions\(\([0-9], Conflict\)\)"#).unwrap();
        assert!(re.is_match(&errmsg));
    } else {
        panic!("should be the Conflict Transactions error");
    }
}

#[test]
fn test_genesis_transaction_fetch() {
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0, Default::default()))
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

    let root_hash = tx.hash().clone();

    let genesis_block = BlockBuilder::default()
        .transaction(tx)
        .with_header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)));

    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (_chain_controller, shared) = start_chain(Some(consensus), false);

    let out_point = OutPoint::new(root_hash, 0);
    let state = shared.chain_state().lock().get_cell_status(&out_point);
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
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
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
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
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
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
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
