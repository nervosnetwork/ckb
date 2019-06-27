use crate::tests::util::{
    create_always_success_tx, create_cellbase, create_multi_outputs_transaction,
    create_transaction, create_transaction_with_out_point, dao_data, start_chain, MockChain,
    MockStore,
};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::cell::{BlockInfo, CellMetaBuilder, CellProvider, CellStatus, UnresolvableError};
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutPoint, CellOutput, OutPoint, TransactionBuilder};
use ckb_core::{capacity_bytes, Bytes, Capacity};
use ckb_dao_utils::genesis_dao_data;
use ckb_shared::error::SharedError;
use ckb_store::ChainStore;
use ckb_test_chain_utils::{build_block, header_builder};
use ckb_traits::ChainProvider;
use numext_fixed_uint::U256;
use std::sync::Arc;

#[test]
fn test_genesis_transaction_spend() {
    let tx = TransactionBuilder::default()
        .witness(Script::default().into_witness())
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

    let dao = genesis_dao_data(&tx).unwrap();

    let genesis_block = build_block!(
        transaction: tx,
        transaction: create_always_success_tx(),
        header_builder:
            header_builder!(difficulty: U256::from(1000u64),
                                        dao: dao,),
    );

    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (chain_controller, shared, parent) = start_chain(Some(consensus));

    let end = 21;

    let mut mock_store = MockStore::new(&parent, shared.store());

    let mut chain = MockChain::new(parent.clone(), shared.consensus());

    for i in 1..end {
        let tx = create_transaction(&root_hash, i as u8);
        root_hash = tx.hash().to_owned();

        // commit txs in block
        chain.gen_block_with_commit_txs(vec![tx], &mut mock_store, false);
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
    let mut mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent.clone(), shared.consensus());
    chain.gen_empty_block(100u64, &mut mock_store);

    let last_cell_base = &chain.tip().cellbase();
    let last_cell_base_hash = last_cell_base.hash().to_owned();
    let tx1 = create_multi_outputs_transaction(&last_cell_base, vec![0], 2, vec![1]);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![2]);
    let tx2_hash = tx2.hash().to_owned();
    let tx2_output = tx2.outputs()[0].clone();

    let txs = vec![tx1, tx2];

    for hash in &[&last_cell_base_hash, &tx1_hash, &tx2_hash] {
        assert_eq!(
            shared
                .lock_chain_state()
                .cell(&OutPoint::new_cell(hash.to_owned().to_owned(), 0)),
            CellStatus::Unknown
        );
    }

    // proposal txs
    chain.gen_block_with_proposal_txs(txs.clone(), &mut mock_store);
    // empty block
    chain.gen_empty_block(100, &mut mock_store);
    // commit txs in block
    chain.gen_block_with_commit_txs(txs.clone(), &mut mock_store, false);

    for block in chain.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }

    // assert last_cell_base_hash is full dead
    assert_eq!(
        shared
            .lock_chain_state()
            .cell(&OutPoint::new_cell(last_cell_base_hash.to_owned(), 0)),
        CellStatus::Unknown
    );

    assert_eq!(
        shared
            .lock_chain_state()
            .cell(&OutPoint::new_cell(tx1_hash.to_owned(), 0)),
        CellStatus::Dead
    );

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
    let (chain_controller, shared, parent) = start_chain(None);
    let mut mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent.clone(), shared.consensus());
    chain.gen_empty_block(100u64, &mut mock_store);

    let last_cell_base = &chain.tip().cellbase();
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_transaction(&tx1_hash, 2);
    let tx3 = create_transaction(&tx1_hash, 3);
    let txs = vec![tx1, tx2, tx3];

    // proposal txs
    chain.gen_block_with_proposal_txs(txs.clone(), &mut mock_store);
    // empty block
    chain.gen_empty_block(100, &mut mock_store);
    // commit txs in block
    chain.gen_block_with_commit_txs(txs.clone(), &mut mock_store, true);

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
    let (chain_controller, shared, parent) = start_chain(None);
    let mut mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent.clone(), shared.consensus());
    chain.gen_empty_block(100u64, &mut mock_store);

    let last_cell_base = &chain.tip().cellbase();
    let tx1 = create_multi_outputs_transaction(&last_cell_base, vec![0], 2, vec![1]);
    let tx1_hash = tx1.hash();
    let tx2 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![1]);
    let tx3 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![2]);
    // proposal txs
    chain.gen_block_with_proposal_txs(vec![tx1.clone(), tx2.clone(), tx3.clone()], &mut mock_store);

    // empty N+1 block
    chain.gen_empty_block(100, &mut mock_store);

    // commit tx1 and tx2 in N+2 block
    chain.gen_block_with_commit_txs(vec![tx1.clone(), tx2.clone()], &mut mock_store, false);

    // commit tx3 in N+3 block
    chain.gen_block_with_commit_txs(vec![tx3.clone()], &mut mock_store, false);

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
    let (chain_controller, shared, parent) = start_chain(None);
    let mut mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent.clone(), shared.consensus());
    chain.gen_empty_block(100u64, &mut mock_store);

    let last_cell_base = &chain.tip().cellbase();
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_transaction(&tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new_cell(tx1_hash.clone(), 1), 3);
    let txs = vec![tx1, tx2, tx3];
    // proposal txs
    chain.gen_block_with_proposal_txs(txs.clone(), &mut mock_store);
    // empty N+1 block
    chain.gen_empty_block(100, &mut mock_store);
    // commit txs in N+2 block
    chain.gen_block_with_commit_txs(txs.clone(), &mut mock_store, true);

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
    let (chain_controller, shared, parent) = start_chain(None);
    let mut mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent.clone(), shared.consensus());
    chain.gen_empty_block(100u64, &mut mock_store);

    let last_cell_base = &chain.tip().cellbase();
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new_cell(tx1_hash.clone(), 1), 3);
    // proposal txs
    chain.gen_block_with_proposal_txs(vec![tx1.clone(), tx2.clone(), tx3.clone()], &mut mock_store);
    // empty N+1 block
    chain.gen_empty_block(100, &mut mock_store);
    // commit tx1 and tx2 in N+2 block
    chain.gen_block_with_commit_txs(vec![tx1.clone(), tx2.clone()], &mut mock_store, false);
    // commit tx3 in N+3 block
    chain.gen_block_with_commit_txs(vec![tx3.clone()], &mut mock_store, true);

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
        .witness(Script::default().into_witness())
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

    let mut mock_store = MockStore::new(&parent, shared.store());
    let mut chain1 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain2 = MockChain::new(parent.clone(), shared.consensus());

    // 100 * 20 = 2000
    for _ in 0..final_number {
        chain1.gen_empty_block_with_difficulty(100u64, &mut mock_store);
    }

    // 99 * 10 + 110 * 10 = 2090
    for i in 0..final_number {
        let j = if i > 10 { 110 } else { 99 };
        chain2.gen_empty_block_with_difficulty(j, &mut mock_store);
    }

    assert!(chain2.total_difficulty() > chain1.total_difficulty());

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

    let mut mock_store = MockStore::new(&parent, shared.store());
    let mut chain1 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain2 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain3 = MockChain::new(parent.clone(), shared.consensus());

    // 100 * 20 = 2000
    for _ in 0..final_number {
        chain1.gen_empty_block_with_difficulty(100u64, &mut mock_store);
    }

    // 50 * 40 = 2000
    for _ in 0..(final_number * 2) {
        chain2.gen_empty_block_with_difficulty(50u64, &mut mock_store);
    }

    // 20 * 100 = 2000
    for _ in 0..(final_number * 5) {
        chain3.gen_empty_block_with_difficulty(20u64, &mut mock_store);
    }

    for chain in vec![chain1.clone(), chain2.clone(), chain3.clone()] {
        for block in chain.blocks() {
            chain_controller
                .process_block(Arc::new(block.clone()), false)
                .expect("process block ok");
        }
    }

    // if total_difficulty equal, we chose block which have smaller hash as best
    assert_eq!(chain1.total_difficulty(), chain2.total_difficulty());
    assert_eq!(chain1.total_difficulty(), chain3.total_difficulty());

    let hash1 = chain1.tip_header().hash();
    let hash2 = chain2.tip_header().hash();
    let hash3 = chain3.tip_header().hash();

    let tips = vec![hash1.clone(), hash2.clone(), hash3.clone()];
    let v = tips.iter().min().unwrap();

    let best = match v {
        hash if hash == hash1 => chain1,
        hash if hash == hash2 => chain2,
        _ => chain3,
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

    let mut mock_store = MockStore::new(&parent, shared.store());
    let mut chain1 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain2 = MockChain::new(parent.clone(), shared.consensus());

    for _ in 1..final_number {
        chain1.gen_empty_block(100u64, &mut mock_store);
    }

    for _ in 1..final_number {
        chain2.gen_empty_block(90u64, &mut mock_store);
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

    assert!(chain1.tip_header().hash() != chain2.tip_header().hash());

    assert_eq!(
        *chain1.blocks()[9].header(),
        shared
            .store()
            .get_ancestor(&chain1.tip_header().hash(), 10)
            .unwrap()
    );

    assert_eq!(
        *chain2.blocks()[9].header(),
        shared
            .store()
            .get_ancestor(&chain2.tip_header().hash(), 10)
            .unwrap()
    );
}

#[test]
fn test_next_epoch_ext() {
    let cellbase = TransactionBuilder::default()
        .witness(Script::default().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .build();
    let dao = genesis_dao_data(&cellbase).unwrap();
    let genesis_block = build_block! {
        header_builder: header_builder!(difficulty: U256::from(1000u64),
                                        dao: dao,),
        transaction: cellbase,
    };

    let mut consensus = Consensus::default().set_genesis_block(genesis_block);
    consensus.genesis_epoch_ext.set_length(400);
    let epoch = consensus.genesis_epoch_ext.clone();

    let (chain_controller, shared, genesis) = start_chain(Some(consensus.clone()));
    let final_number = shared.consensus().genesis_epoch_ext().length();

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let mut parent = genesis.clone();
    let mut last_epoch = epoch.clone();

    let mut mock_store = MockStore::new(&parent, shared.store());

    for _ in 1..final_number - 1 {
        let epoch = shared
            .next_epoch_ext(&last_epoch, &parent)
            .unwrap_or(last_epoch);

        let transactions = vec![create_cellbase(
            &mut mock_store,
            shared.consensus(),
            &parent,
        )];
        let dao = dao_data(
            shared.consensus(),
            &parent,
            &transactions,
            &mut mock_store,
            false,
        );

        let new_block = build_block!(
            from_header_builder: {
                parent_hash: parent.hash().to_owned(),
                number: parent.number() + 1,
                timestamp: parent.timestamp() + 20_000,
                difficulty: epoch.difficulty().clone(),
                dao: dao,
            },
            transactions: transactions,
        );

        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain1.push(new_block.clone());
        mock_store.insert_block(&new_block, &epoch);
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
            uncles.push(chain1[i as usize].clone());
        }

        let transactions = vec![create_cellbase(
            &mut mock_store,
            shared.consensus(),
            &parent,
        )];
        let dao = dao_data(
            shared.consensus(),
            &parent,
            &transactions,
            &mut mock_store,
            false,
        );

        let new_block = build_block!(
            from_header_builder: {
                parent_hash: parent.hash().to_owned(),
                number: parent.number() + 1,
                timestamp: parent.timestamp() + 20_000,
                difficulty: epoch.difficulty().clone(),
                dao: dao,
            },
            transactions: transactions,
            uncles: uncles.clone(),
        );

        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain2.push(new_block.clone());
        mock_store.insert_block(&new_block, &epoch);
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
        // epoch_duration_target 14400000
        // target_recip 20
        // last_duration 7980000

        // (Diff_last * o_last) / o
        // (25 * 1000 * 20) / 400
        assert_eq!(epoch.difficulty(), &U256::from(1250u64));

        // ((Cu_last + Cm_last) * L * Cm_last) / ((u + 1) * Cu_last * L_last)
        // ((25 + 400) * 14400000 * 400) / (( 20 + 1)* 25 * 7980000)
        assert_eq!(epoch.length(), 584);

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
            uncles.push(chain1[i as usize].clone());
        }

        let transactions = vec![create_cellbase(
            &mut mock_store,
            shared.consensus(),
            &parent,
        )];
        let dao = dao_data(
            shared.consensus(),
            &parent,
            &transactions,
            &mut mock_store,
            false,
        );

        let new_block = build_block!(
            from_header_builder: {
                parent_hash: parent.hash().to_owned(),
                number: parent.number() + 1,
                timestamp: parent.timestamp() + 20_000,
                difficulty: epoch.difficulty().clone(),
                dao: dao,
            },
            transactions: transactions,
            uncles: uncles,
        );

        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain2.push(new_block.clone());
        mock_store.insert_block(&new_block, &epoch);
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
            uncles.push(chain1[i as usize].clone());
        }

        let transactions = vec![create_cellbase(
            &mut mock_store,
            shared.consensus(),
            &parent,
        )];
        let dao = dao_data(
            shared.consensus(),
            &parent,
            &transactions,
            &mut mock_store,
            false,
        );

        let new_block = build_block!(
            from_header_builder: {
                parent_hash: parent.hash().to_owned(),
                number: parent.number() + 1,
                timestamp: parent.timestamp() + 20_000,
                difficulty: epoch.difficulty().clone(),
                dao: dao,
            },
            transactions: transactions,
            uncles: uncles,
        );

        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain2.push(new_block.clone());
        mock_store.insert_block(&new_block, &epoch);
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
