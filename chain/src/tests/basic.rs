use crate::tests::util::{
    create_transaction, create_transaction_with_out_point, gen_block, start_chain,
};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::block::BlockBuilder;
use ckb_core::cell::{CellMeta, CellProvider, CellStatus, UnresolvableError};
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutPoint, CellOutput, OutPoint, TransactionBuilder};
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

    let mut root_hash = tx.hash().to_owned();

    let genesis_tx_hash = root_hash.clone();

    let genesis_block = BlockBuilder::default()
        .transaction(tx)
        .header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)))
        .build();

    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (chain_controller, shared) = start_chain(Some(consensus));

    let end = 21;

    let mut blocks1: Vec<Block> = vec![];
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for i in 1..end {
        let difficulty = parent.difficulty().to_owned();
        let tx = create_transaction(&root_hash, i as u8);
        root_hash = tx.hash().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(1u64),
            vec![tx],
            vec![],
            vec![],
        );
        blocks1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    for block in &blocks1[0..10] {
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
    let (chain_controller, shared) = start_chain(None);
    let mut chain: Vec<Block> = Vec::new();
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }

    let last_cell_base = &chain.last().unwrap().transactions()[0];
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
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            txs.clone(),
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }
    // empty N+1 block
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }
    // commit txs in N+2 block
    {
        let difficulty = parent.difficulty().to_owned();
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
        CellStatus::live_cell(CellMeta {
            cell_output: None,
            out_point: CellOutPoint {
                tx_hash: tx2_hash.to_owned(),
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
    let (chain_controller, shared) = start_chain(None);
    let mut chain: Vec<Block> = Vec::new();
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }

    let last_cell_base = &chain.last().unwrap().transactions()[0];
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_transaction(&tx1_hash, 2);
    let tx3 = create_transaction(&tx1_hash, 3);
    let txs = vec![tx1, tx2, tx3];
    // proposal txs
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            txs.clone(),
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }
    // empty N+1 block
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }
    // commit txs in N+2 block
    {
        let difficulty = parent.difficulty().to_owned();
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
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }
    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Dead(OutPoint::new_cell(
            tx1_hash.to_owned(),
            0
        ))),
        chain_controller
            .process_block(Arc::new(chain[3].clone()), true)
            .unwrap_err()
            .downcast()
            .unwrap()
    );
}

#[test]
fn test_transaction_conflict_in_different_blocks() {
    let (chain_controller, shared) = start_chain(None);
    let mut chain: Vec<Block> = Vec::new();
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }

    let last_cell_base = &chain.last().unwrap().transactions()[0];
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(tx1_hash, 2);
    let tx3 = create_transaction(tx1_hash, 3);
    // proposal txs
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![tx1.clone(), tx2.clone(), tx3.clone()],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }
    // empty N+1 block
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }
    // commit tx1 and tx2 in N+2 block
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![tx1.clone(), tx2.clone()],
            vec![],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }
    // commit tx3 in N+3 block
    {
        let difficulty = parent.difficulty().to_owned();
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
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }
    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Dead(OutPoint::new_cell(
            tx1_hash.to_owned(),
            0
        ))),
        chain_controller
            .process_block(Arc::new(chain[4].clone()), true)
            .unwrap_err()
            .downcast()
            .unwrap()
    );
}

#[test]
fn test_invalid_out_point_index_in_same_block() {
    let (chain_controller, shared) = start_chain(None);
    let mut chain: Vec<Block> = Vec::new();
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }

    let last_cell_base = &chain.last().unwrap().transactions()[0];
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_transaction(&tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new_cell(tx1_hash.clone(), 1), 3);
    let txs = vec![tx1, tx2, tx3];
    // proposal txs
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            txs.clone(),
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }
    // empty N+1 block
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }
    // commit txs in N+2 block
    {
        let difficulty = parent.difficulty().to_owned();
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
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }
    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Unknown(vec![OutPoint::new_cell(
            tx1_hash.to_owned(),
            1,
        )])),
        chain_controller
            .process_block(Arc::new(chain[3].clone()), true)
            .unwrap_err()
            .downcast()
            .unwrap()
    );
}

#[test]
fn test_invalid_out_point_index_in_different_blocks() {
    let (chain_controller, shared) = start_chain(None);
    let mut chain: Vec<Block> = Vec::new();
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }

    let last_cell_base = &chain.last().unwrap().transactions()[0];
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new_cell(tx1_hash.clone(), 1), 3);
    // proposal txs
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![tx1.clone(), tx2.clone(), tx3.clone()],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }
    // empty N+1 block
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }
    // commit tx1 and tx2 in N+2 block
    {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![tx1.clone(), tx2.clone()],
            vec![],
            vec![],
        );
        parent = new_block.header().to_owned();
        chain.push(new_block);
    }
    // commit tx3 in N+3 block
    {
        let difficulty = parent.difficulty().to_owned();
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
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }

    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Unknown(vec![OutPoint::new_cell(
            tx1_hash.to_owned(),
            1,
        )])),
        chain_controller
            .process_block(Arc::new(chain[4].clone()), true)
            .unwrap_err()
            .downcast()
            .unwrap()
    );
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

    let root_hash = tx.hash().to_owned();

    let genesis_block = BlockBuilder::default()
        .transaction(tx)
        .header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)))
        .build();

    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (_chain_controller, shared) = start_chain(Some(consensus));

    let out_point = OutPoint::new_cell(root_hash, 0);
    let state = shared.lock_chain_state().cell(&out_point);
    assert!(state.is_live());
}

#[test]
fn test_chain_fork_by_total_difficulty() {
    let (chain_controller, shared) = start_chain(None);
    let final_number = 20;

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        chain1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for i in 1..final_number {
        let difficulty = parent.difficulty().to_owned();
        let j = if i > 10 { 110 } else { 99 };
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(j as u32),
            vec![],
            vec![],
            vec![],
        );
        chain2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    for block in &chain1 {
        chain_controller
            .process_block(Arc::new(block.clone()), false)
            .expect("process block ok");
    }

    for block in &chain2 {
        chain_controller
            .process_block(Arc::new(block.clone()), false)
            .expect("process block ok");
    }
    assert_eq!(
        shared.block_hash(8),
        chain2.get(7).map(|b| b.header().hash().to_owned())
    );
}

#[test]
fn test_chain_fork_by_hash() {
    let (chain_controller, shared) = start_chain(None);
    let final_number = 20;

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        chain1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        chain2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    for block in &chain1 {
        chain_controller
            .process_block(Arc::new(block.clone()), false)
            .expect("process block ok");
    }

    for block in &chain2 {
        chain_controller
            .process_block(Arc::new(block.clone()), false)
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
    assert_eq!(
        shared.block_hash(8),
        best.get(7).map(|b| b.header().hash().to_owned())
    );
    assert_eq!(
        shared.block_hash(19),
        best.get(18).map(|b| b.header().hash().to_owned())
    );
}

#[test]
fn test_chain_get_ancestor() {
    let (chain_controller, shared) = start_chain(None);
    let final_number = 20;

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        chain1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        chain2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    for block in &chain1 {
        chain_controller
            .process_block(Arc::new(block.clone()), false)
            .expect("process block ok");
    }

    for block in &chain2 {
        chain_controller
            .process_block(Arc::new(block.clone()), false)
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
fn test_next_epoch_ext() {
    let genesis_block = BlockBuilder::default()
        .header_builder(HeaderBuilder::default().difficulty(U256::from(1000u64)))
        .build();
    let mut consensus = Consensus::default().set_genesis_block(genesis_block);
    consensus.genesis_epoch_ext.set_length(400);
    let epoch = consensus.genesis_epoch_ext.clone();

    let (chain_controller, shared) = start_chain(Some(consensus.clone()));
    let final_number = shared.consensus().genesis_epoch_ext().length();

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
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

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
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
        let total_uncles_count = shared.block_ext(&tip.hash()).unwrap().total_uncles_count;
        assert_eq!(total_uncles_count, 25);

        let epoch = shared
            .next_epoch_ext(chain_state.current_epoch_ext(), &tip)
            .unwrap();

        // last_uncles_count 25
        // last_epoch_length 200
        // epoch_duration_target 14400000
        // target_recip 20
        // last_duration 7980000

        // (25 * 1000 * 20) / 400
        assert_eq!(epoch.difficulty(), &U256::from(1250u64));

        /// ((25 + 400) * 14400000 * 400) / (( 20 + 1)* 25 * 7980000)
        assert_eq!(epoch.length(), 584);

        let consensus = shared.consensus();

        let epoch_reward = consensus.epoch_reward();
        let start_reward = Capacity::shannons(
            epoch.remainder_reward().as_u64() + epoch_reward.as_u64() / epoch.length(),
        );
        let block_reward = Capacity::shannons(epoch_reward.as_u64() / epoch.length());

        // block_reward 856164383561
        // remainder_reward 376
        assert_eq!(
            epoch.block_reward(400).unwrap(),
            start_reward // apacity::shannons(844594594946)
        );
        assert_eq!(
            epoch.block_reward(401).unwrap(),
            block_reward // Capacity::shannons(844594594594)
        );
    }

    let (chain_controller, shared) = start_chain(Some(consensus.clone()));
    let mut chain2: Vec<Block> = Vec::new();
    for i in 1..final_number - 1 {
        chain_controller
            .process_block(Arc::new(chain1[(i - 1) as usize].clone()), false)
            .expect("process block ok");
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
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
        let total_uncles_count = shared.block_ext(&tip.hash()).unwrap().total_uncles_count;
        assert_eq!(total_uncles_count, 10);

        let epoch = shared
            .next_epoch_ext(chain_state.current_epoch_ext(), &tip)
            .unwrap();

        assert_eq!(epoch.difficulty(), &U256::from(1000u64));
    }

    let (chain_controller, shared) = start_chain(Some(consensus.clone()));
    let mut chain2: Vec<Block> = Vec::new();
    for i in 1..final_number - 1 {
        chain_controller
            .process_block(Arc::new(chain1[(i - 1) as usize].clone()), false)
            .expect("process block ok");
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
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
        let total_uncles_count = shared.block_ext(&tip.hash()).unwrap().total_uncles_count;
        assert_eq!(total_uncles_count, 150);

        let epoch = shared
            .next_epoch_ext(chain_state.current_epoch_ext(), &tip)
            .unwrap();
        // max[150 * 10 * 1000 / 200, 2 * 1000]
        assert_eq!(epoch.difficulty(), &U256::from(2000u64));
    }
}
