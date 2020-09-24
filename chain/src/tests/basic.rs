use crate::tests::util::{
    create_always_success_tx, create_cellbase, create_multi_outputs_transaction,
    create_transaction, create_transaction_with_out_point, dao_data, start_chain, MockChain,
    MockStore,
};
use crate::{chain::ChainController, switch::Switch};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_dao_utils::genesis_dao_data;
use ckb_error::assert_error_eq;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_types::core::error::OutPointError;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes,
        cell::{CellMeta, CellProvider, CellStatus},
        BlockBuilder, BlockView, Capacity, EpochExt, HeaderView, TransactionBuilder,
        TransactionInfo,
    },
    packed::{CellInput, CellOutputBuilder, OutPoint, Script},
    utilities::{compact_to_difficulty, difficulty_to_compact},
    U256,
};
use std::sync::Arc;

#[test]
fn repeat_process_block() {
    let (chain_controller, shared, parent) = start_chain(None);
    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent, shared.consensus());
    chain.gen_empty_block_with_nonce(100u128, &mock_store);
    let block = Arc::new(chain.blocks().last().unwrap().clone());

    assert!(chain_controller
        .process_block(Arc::clone(&block))
        .expect("process block ok"));
    assert_eq!(
        shared
            .store()
            .get_block_ext(&block.header().hash())
            .unwrap()
            .verified,
        Some(true)
    );

    assert!(!chain_controller
        .process_block(Arc::clone(&block))
        .expect("process block ok"));
    assert_eq!(
        shared
            .store()
            .get_block_ext(&block.header().hash())
            .unwrap()
            .verified,
        Some(true)
    );
}

#[test]
fn test_genesis_transaction_spend() {
    // let data: Vec<packed::Bytes> = ;
    let tx = TransactionBuilder::default()
        .witness(Script::default().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs(vec![
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(100_000_000).pack())
                .build();
            100
        ])
        .outputs_data(vec![Bytes::new(); 100].pack())
        .build();
    let always_success_tx = create_always_success_tx();

    let mut root_hash = tx.hash();

    let genesis_tx_hash = root_hash.clone();

    let dao = genesis_dao_data(vec![&tx, &always_success_tx]).unwrap();

    let genesis_block = BlockBuilder::default()
        .transaction(tx)
        .transaction(always_success_tx)
        .compact_target(difficulty_to_compact(U256::from(1000u64)).pack())
        .dao(dao)
        .build();

    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis_block)
        .build();
    let (chain_controller, shared, parent) = start_chain(Some(consensus));

    let end = 21;

    let mock_store = MockStore::new(&parent, shared.store());

    let mut chain = MockChain::new(parent, shared.consensus());

    for i in 1..end {
        let tx = create_transaction(&root_hash, i as u8);
        root_hash = tx.hash();

        // commit txs in block
        chain.gen_block_with_commit_txs(vec![tx], &mock_store, false);
    }

    for block in &chain.blocks()[0..10] {
        assert!(chain_controller
            .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
            .is_ok());
    }

    assert_eq!(
        shared
            .snapshot()
            .cell(&OutPoint::new(genesis_tx_hash, 0), false),
        CellStatus::Unknown
    );
}

#[test]
fn test_transaction_spend_in_same_block() {
    let (chain_controller, shared, parent) = start_chain(None);
    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent, shared.consensus());
    chain.gen_empty_block(&mock_store);

    let last_cellbase = &shared.consensus().genesis_block().transactions()[1];
    let last_cellbase_hash = last_cellbase.hash();
    let tx1 = create_multi_outputs_transaction(&last_cellbase, vec![0], 2, vec![1]);
    let tx1_hash = tx1.hash();
    let tx2 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![2]);
    let tx2_hash = tx2.hash();
    let tx2_output = tx2.outputs().get(0).expect("outputs index 0");
    let tx2_output_data = tx2.outputs_data().get(0).expect("outputs_data index 0");

    let txs = vec![tx1, tx2];

    for hash in &[&tx1_hash, &tx2_hash] {
        assert_eq!(
            shared
                .snapshot()
                .cell(&OutPoint::new(hash.to_owned().to_owned(), 0), false),
            CellStatus::Unknown
        );
    }

    // proposal txs
    chain.gen_block_with_proposal_txs(txs.clone(), &mock_store);
    // empty block
    chain.gen_empty_block(&mock_store);
    // commit txs in block
    chain.gen_block_with_commit_txs(txs, &mock_store, false);
    let (parent_hash4, parent_number4) = {
        chain
            .blocks()
            .last()
            .map(|block| (block.header().hash(), block.header().number()))
            .unwrap()
    };

    for block in chain.blocks() {
        chain_controller
            .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_EPOCH)
            .expect("process block ok");
    }

    // assert last_cellbase_hash is full dead
    assert_eq!(
        shared
            .snapshot()
            .cell(&OutPoint::new(last_cellbase_hash, 0), false),
        CellStatus::Unknown
    );

    assert_eq!(
        shared.snapshot().cell(&OutPoint::new(tx1_hash, 0), false),
        CellStatus::Unknown
    );

    let epoch = mock_store
        .0
        .get_block_epoch_index(&parent_hash4)
        .and_then(|index| mock_store.0.get_epoch_ext(&index))
        .unwrap();

    assert_eq!(
        shared
            .snapshot()
            .cell(&OutPoint::new(tx2_hash.clone(), 0), false),
        CellStatus::live_cell(CellMeta {
            cell_output: tx2_output,
            data_bytes: tx2_output_data.len() as u64,
            out_point: OutPoint::new(tx2_hash, 0),
            transaction_info: Some(TransactionInfo::new(
                parent_number4,
                epoch.number_with_fraction(parent_number4),
                parent_hash4,
                2
            )),
            mem_cell_data: None,
        })
    );
}

#[test]
fn test_transaction_conflict_in_same_block() {
    let (chain_controller, shared, parent) = start_chain(None);
    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent, shared.consensus());
    chain.gen_empty_block(&mock_store);

    let last_cellbase = &shared.consensus().genesis_block().transactions()[1];
    let tx1 = create_transaction(&last_cellbase.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(&tx1_hash, 2);
    let tx3 = create_transaction(&tx1_hash, 3);
    let txs = vec![tx1, tx2, tx3];

    // proposal txs
    chain.gen_block_with_proposal_txs(txs.clone(), &mock_store);
    // empty block
    chain.gen_empty_block(&mock_store);
    // commit txs in block
    chain.gen_block_with_commit_txs(txs, &mock_store, true);

    for block in chain.blocks().iter().take(3) {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }
    assert_error_eq!(
        OutPointError::Dead(OutPoint::new(tx1_hash, 0)),
        chain_controller
            .process_block(Arc::new(chain.blocks()[3].clone()))
            .unwrap_err(),
    );
}

#[test]
fn test_transaction_conflict_in_different_blocks() {
    let (chain_controller, shared, parent) = start_chain(None);
    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent, shared.consensus());
    chain.gen_empty_block(&mock_store);

    let last_cellbase = &shared.consensus().genesis_block().transactions()[1];
    let tx1 = create_multi_outputs_transaction(&last_cellbase, vec![0], 2, vec![1]);
    let tx1_hash = tx1.hash();
    let tx2 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![1]);
    let tx3 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![2]);
    // proposal txs
    chain.gen_block_with_proposal_txs(vec![tx1.clone(), tx2.clone(), tx3.clone()], &mock_store);

    // empty N+1 block
    chain.gen_empty_block(&mock_store);

    // commit tx1 and tx2 in N+2 block
    chain.gen_block_with_commit_txs(vec![tx1, tx2], &mock_store, false);

    // commit tx3 in N+3 block
    chain.gen_block_with_commit_txs(vec![tx3], &mock_store, false);

    for block in chain.blocks().iter().take(4) {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }
    assert_error_eq!(
        OutPointError::Unknown(vec![OutPoint::new(tx1_hash, 0)]),
        chain_controller
            .process_block(Arc::new(chain.blocks()[4].clone()))
            .unwrap_err(),
    );
}

#[test]
fn test_invalid_out_point_index_in_same_block() {
    let (chain_controller, shared, parent) = start_chain(None);
    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent, shared.consensus());
    chain.gen_empty_block(&mock_store);

    let last_cellbase = &shared.consensus().genesis_block().transactions()[1];
    let tx1 = create_transaction(&last_cellbase.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(&tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new(tx1_hash.clone(), 1), 3);
    let txs = vec![tx1, tx2, tx3];
    // proposal txs
    chain.gen_block_with_proposal_txs(txs.clone(), &mock_store);
    // empty N+1 block
    chain.gen_empty_block(&mock_store);
    // commit txs in N+2 block
    chain.gen_block_with_commit_txs(txs, &mock_store, true);

    for block in chain.blocks().iter().take(3) {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }
    assert_error_eq!(
        OutPointError::Unknown(vec![OutPoint::new(tx1_hash, 1)]),
        chain_controller
            .process_block(Arc::new(chain.blocks()[3].clone()))
            .unwrap_err(),
    );
}

#[test]
fn test_invalid_out_point_index_in_different_blocks() {
    let (chain_controller, shared, parent) = start_chain(None);
    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent, shared.consensus());
    chain.gen_empty_block_with_nonce(100u128, &mock_store);

    let last_cellbase = &shared.consensus().genesis_block().transactions()[1];
    let tx1 = create_transaction(&last_cellbase.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(&tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new(tx1_hash.clone(), 1), 3);
    // proposal txs
    chain.gen_block_with_proposal_txs(vec![tx1.clone(), tx2.clone(), tx3.clone()], &mock_store);
    // empty N+1 block
    chain.gen_empty_block_with_nonce(100u128, &mock_store);
    // commit tx1 and tx2 in N+2 block
    chain.gen_block_with_commit_txs(vec![tx1, tx2], &mock_store, false);
    // commit tx3 in N+3 block
    chain.gen_block_with_commit_txs(vec![tx3], &mock_store, true);

    for block in chain.blocks().iter().take(4) {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }

    assert_error_eq!(
        OutPointError::Unknown(vec![OutPoint::new(tx1_hash, 1)]),
        chain_controller
            .process_block(Arc::new(chain.blocks()[4].clone()))
            .unwrap_err(),
    );
}

#[test]
fn test_genesis_transaction_fetch() {
    let tx = TransactionBuilder::default()
        .witness(Script::default().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .outputs(vec![
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(100_000_000).pack())
                .lock(Script::default())
                .build();
            100
        ])
        .outputs_data(vec![Bytes::new(); 100].pack())
        .build();

    let root_hash = tx.hash();

    let genesis_block = BlockBuilder::default()
        .transaction(tx)
        .compact_target(difficulty_to_compact(U256::from(1000u64)).pack())
        .build();

    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis_block)
        .build();
    let (_chain_controller, shared, _parent) = start_chain(Some(consensus));

    let out_point = OutPoint::new(root_hash, 0);
    let state = shared.snapshot().cell(&out_point, false);
    assert!(state.is_live());
}

#[test]
fn test_chain_fork_by_total_difficulty() {
    let (chain_controller, shared, parent) = start_chain(None);
    let final_number = 20;

    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain1 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain2 = MockChain::new(parent, shared.consensus());

    // 100 * 20 = 2000
    for _ in 0..final_number {
        chain1.gen_empty_block_with_diff(100u64, &mock_store);
    }

    // 99 * 10 + 110 * 10 = 2090
    for i in 0..final_number {
        let j = if i > 10 { 110 } else { 99 };
        chain2.gen_empty_block_with_diff(j, &mock_store);
    }

    assert!(chain2.total_difficulty() > chain1.total_difficulty());

    for block in chain1.blocks() {
        chain_controller
            .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
            .expect("process block ok");
    }

    for block in chain2.blocks() {
        chain_controller
            .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
            .expect("process block ok");
    }
    assert_eq!(
        shared.store().get_block_hash(8),
        chain2.blocks().get(7).map(|b| b.header().hash())
    );
}

#[test]
fn test_chain_fork_by_first_received() {
    let (chain_controller, shared, parent) = start_chain(None);
    let final_number = 20;

    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain1 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain2 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain3 = MockChain::new(parent, shared.consensus());

    // 100 * 20 = 2000
    for _ in 0..final_number {
        chain1.gen_empty_block_with_diff(100u64, &mock_store);
    }

    // 50 * 40 = 2000
    for _ in 0..(final_number * 2) {
        chain2.gen_empty_block_with_diff(50u64, &mock_store);
    }

    // 20 * 100 = 2000
    for _ in 0..(final_number * 5) {
        chain3.gen_empty_block_with_diff(20u64, &mock_store);
    }

    for chain in vec![chain1.clone(), chain2.clone(), chain3.clone()] {
        for block in chain.blocks() {
            chain_controller
                .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
                .expect("process block ok");
        }
    }

    // if total_difficulty equal, we chose block which have smaller hash as best
    assert_eq!(chain1.total_difficulty(), chain2.total_difficulty());
    assert_eq!(chain1.total_difficulty(), chain3.total_difficulty());

    // fist received will be the main chain
    assert_eq!(
        shared.store().get_block_hash(8),
        chain1.blocks().get(7).map(|b| b.header().hash())
    );
    assert_eq!(
        shared.store().get_block_hash(19),
        chain1.blocks().get(18).map(|b| b.header().hash())
    );
}

fn prepare_context_chain(
    consensus: Consensus,
    orphan_count: u64,
    timestep: u64,
) -> (ChainController, Shared, HeaderView, EpochExt) {
    let epoch = consensus.genesis_epoch_ext.clone();
    let (chain_controller, shared, genesis) = start_chain(Some(consensus));
    let final_number = shared.consensus().genesis_epoch_ext().length();

    let mut chain1: Vec<BlockView> = Vec::new();
    let mut chain2: Vec<BlockView> = Vec::new();

    let mut parent = genesis.clone();
    let mut last_epoch = epoch.clone();

    let mock_store = MockStore::new(&parent, shared.store());

    for _ in 1..final_number - 1 {
        let epoch = shared
            .snapshot()
            .next_epoch_ext(shared.consensus(), &last_epoch, &parent)
            .unwrap_or(last_epoch);

        let transactions = vec![create_cellbase(&mock_store, shared.consensus(), &parent)];
        let dao = dao_data(
            shared.consensus(),
            &parent,
            &transactions,
            &mock_store,
            false,
        );

        let new_block = BlockBuilder::default()
            .parent_hash(parent.hash())
            .number((parent.number() + 1).pack())
            .timestamp((parent.timestamp() + timestep).pack())
            .compact_target(epoch.compact_target().pack())
            .transactions(transactions)
            .dao(dao)
            .build();

        chain_controller
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .expect("process block ok");
        chain1.push(new_block.clone());
        mock_store.insert_block(&new_block, &epoch);
        parent = new_block.header().clone();
        last_epoch = epoch;
    }

    parent = genesis.clone();
    let mut last_epoch = epoch;
    for i in 1..final_number {
        let epoch = shared
            .snapshot()
            .next_epoch_ext(shared.consensus(), &last_epoch, &parent)
            .unwrap_or(last_epoch);
        let mut uncles = vec![];
        if i < orphan_count {
            uncles.push(chain1[i as usize].as_uncle());
        }

        let transactions = vec![create_cellbase(&mock_store, shared.consensus(), &parent)];
        let dao = dao_data(
            shared.consensus(),
            &parent,
            &transactions,
            &mock_store,
            false,
        );

        let new_block = BlockBuilder::default()
            .parent_hash(parent.hash())
            .uncles(uncles)
            .number((parent.number() + 1).pack())
            .timestamp((parent.timestamp() + timestep).pack())
            .compact_target(epoch.compact_target().pack())
            .transactions(transactions)
            .dao(dao)
            .build();

        chain_controller
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .expect("process block ok");
        chain2.push(new_block.clone());
        mock_store.insert_block(&new_block, &epoch);
        parent = new_block.header().clone();
        last_epoch = epoch;
    }
    (chain_controller, shared, genesis, last_epoch)
}

#[test]
fn test_epoch_hash_rate_dampening() {
    let cellbase = TransactionBuilder::default()
        .witness(Script::default().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .build();
    let dao = genesis_dao_data(vec![&cellbase]).unwrap();
    let genesis_block = BlockBuilder::default()
        .compact_target(difficulty_to_compact(U256::from(1000u64)).pack())
        .transaction(cellbase)
        .dao(dao)
        .build();

    // last_difficulty 1000
    // last_uncles_count 25
    // last_epoch_length 400
    // last_duration 7980
    let mut consensus = ConsensusBuilder::default()
        .genesis_block(genesis_block.clone())
        .build();
    consensus.genesis_epoch_ext.set_length(400);
    consensus
        .genesis_epoch_ext
        .set_previous_epoch_hash_rate(U256::from(10u64));
    let (_chain_controller, shared, _genesis, _last_epoch) =
        prepare_context_chain(consensus, 26, 20_000);

    {
        let snapshot = shared.snapshot();
        let tip = snapshot.tip_header().clone();
        let total_uncles_count = snapshot
            .get_block_ext(&tip.hash())
            .unwrap()
            .total_uncles_count;
        assert_eq!(total_uncles_count, 25);

        let epoch = snapshot
            .next_epoch_ext(shared.consensus(), snapshot.epoch_ext(), &tip)
            .unwrap();

        // last_epoch_previous_epoch_hash_rate 10
        // HPS  = dampen(last_difficulty * (last_epoch_length + last_uncles_count) / last_duration)
        // 1000 *( 400 + 25) / 7980 = 53
        // TAU = 2
        // dampen(53) = 10 * 2
        assert_eq!(
            epoch.previous_epoch_hash_rate(),
            &U256::from(20u64),
            "previous_epoch_hash_rate {}",
            epoch.previous_epoch_hash_rate()
        );
    }

    let mut consensus = ConsensusBuilder::default()
        .genesis_block(genesis_block)
        .build();
    consensus.genesis_epoch_ext.set_length(400);
    consensus
        .genesis_epoch_ext
        .set_previous_epoch_hash_rate(U256::from(200u64));
    let (_chain_controller, shared, _genesis, _last_epoch) =
        prepare_context_chain(consensus, 26, 20_000);

    {
        let snapshot = shared.snapshot();
        let tip = snapshot.tip_header().clone();
        let total_uncles_count = snapshot
            .get_block_ext(&tip.hash())
            .unwrap()
            .total_uncles_count;
        assert_eq!(total_uncles_count, 25);

        let epoch = snapshot
            .next_epoch_ext(shared.consensus(), snapshot.epoch_ext(), &tip)
            .unwrap();

        // last_epoch_previous_epoch_hash_rate 200
        // HPS  = dampen(last_difficulty * (last_epoch_length + last_uncles_count) / last_duration)
        // 1000 *( 400 + 25) / 7980 = 53
        // TAU = 2
        // dampen(53) = 200 / 2
        assert_eq!(
            epoch.previous_epoch_hash_rate(),
            &U256::from(100u64),
            "previous_epoch_hash_rate {}",
            epoch.previous_epoch_hash_rate()
        );
    }
}

#[test]
fn test_orphan_rate_estimation_overflow() {
    let cellbase = TransactionBuilder::default()
        .witness(Script::default().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .build();
    let dao = genesis_dao_data(vec![&cellbase]).unwrap();

    let genesis_block = BlockBuilder::default()
        .compact_target(difficulty_to_compact(U256::from(1000u64)).pack())
        .transaction(cellbase)
        .dao(dao)
        .build();

    let mut consensus = ConsensusBuilder::default()
        .genesis_block(genesis_block)
        .build();
    consensus.genesis_epoch_ext.set_length(400);

    // last_difficulty 1000
    // last_epoch_length 400
    // epoch_duration_target 14400
    // orphan_rate_target 1/40
    // last_duration 798000
    // last_uncles_count 150
    let (_chain_controller, shared, _genesis, _last_epoch) =
        prepare_context_chain(consensus, 151, 2_000_000);
    {
        let snapshot = shared.snapshot();
        let tip = snapshot.tip_header().clone();
        let total_uncles_count = snapshot
            .get_block_ext(&tip.hash())
            .unwrap()
            .total_uncles_count;
        assert_eq!(total_uncles_count, 150);

        let epoch = snapshot
            .next_epoch_ext(shared.consensus(), snapshot.epoch_ext(), &tip)
            .unwrap();

        assert_eq!(epoch.length(), 300, "epoch length {}", epoch.length());

        // orphan_rate_estimation (22/399 - 1) overflow
        // max((400 + 150) * 1000 / 798000, 1)  last_epoch_hash_rate 1
        // 14400 * 40 / (41 * 300)
        assert_eq!(
            epoch.compact_target(),
            difficulty_to_compact(U256::from(46u64)),
            "epoch compact_target {}",
            compact_to_difficulty(epoch.compact_target())
        );
    }
}

#[test]
fn test_next_epoch_ext() {
    let cellbase = TransactionBuilder::default()
        .witness(Script::default().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .build();
    let dao = genesis_dao_data(vec![&cellbase]).unwrap();
    let genesis_block = BlockBuilder::default()
        .compact_target(difficulty_to_compact(U256::from(1000u64)).pack())
        .transaction(cellbase)
        .dao(dao)
        .build();

    let mut consensus = ConsensusBuilder::default()
        .genesis_block(genesis_block)
        .build();
    let remember_primary_reward = consensus.genesis_epoch_ext.primary_reward();
    consensus.genesis_epoch_ext.set_length(400);
    consensus
        .genesis_epoch_ext
        .set_primary_reward(remember_primary_reward);

    // last_difficulty 1000
    // last_epoch_length 400
    // epoch_duration_target 14400
    // orphan_rate_target 1/40
    // last_duration 7980
    let (_chain_controller, shared, _genesis, _last_epoch) =
        prepare_context_chain(consensus.clone(), 13, 20_000);
    {
        let snapshot = shared.snapshot();
        let tip = snapshot.tip_header().clone();
        let total_uncles_count = snapshot
            .get_block_ext(&tip.hash())
            .unwrap()
            .total_uncles_count;
        assert_eq!(total_uncles_count, 12);

        let epoch = snapshot
            .next_epoch_ext(shared.consensus(), snapshot.epoch_ext(), &tip)
            .unwrap();

        // last_uncles_count 12
        // HPS  = dampen(last_difficulty * (last_epoch_length + last_uncles_count) / last_duration)
        assert_eq!(
            epoch.previous_epoch_hash_rate(),
            &U256::from(51u64),
            "previous_epoch_hash_rate {}",
            epoch.previous_epoch_hash_rate()
        );

        // C_i+1,m = (o_ideal * (1 + o_i ) * L_ideal × C_i,m) / (o_i * (1 + o_ideal ) * L_i)
        // (1/40 * (1+ 12/400) * 14400 * 400) / (12 / 400 * ( 1+ 1/40) * 7980)
        // (412 * 14400 * 400) / (12 * 41 * 7980)
        assert_eq!(epoch.length(), 604, "epoch length {}", epoch.length());

        // None of the edge cases is triggered
        // Diff_i+1 = (HPS_i · L_ideal) / (1 + 0_i+1 ) * C_i+1,m
        // (51 * 14400) / ((1 + 1/20) * 604)
        // (40 * 51 * 14400) / (41 * 604)
        assert_eq!(
            epoch.compact_target(),
            difficulty_to_compact(U256::from(1186u64)),
            "epoch compact_target {}",
            compact_to_difficulty(epoch.compact_target())
        );

        let consensus = shared.consensus();
        let epoch_reward = consensus.primary_epoch_reward(epoch.number());
        let block_reward = Capacity::shannons(epoch_reward.as_u64() / epoch.length());
        let block_reward_plus_one = Capacity::shannons(block_reward.as_u64() + 1);
        let bound = 400 + epoch.remainder_reward().as_u64();

        // block_reward 428082191780
        // remainder_reward 960
        assert_eq!(epoch.block_reward(400).unwrap(), block_reward_plus_one);
        assert_eq!(
            epoch.block_reward(bound - 1).unwrap(),
            block_reward_plus_one
        );
        assert_eq!(epoch.block_reward(bound).unwrap(), block_reward);
    }

    let (_chain_controller, shared, _genesis, _last_epoch) =
        prepare_context_chain(consensus.clone(), 6, 20_000);
    {
        let snapshot = shared.snapshot();
        let tip = snapshot.tip_header().clone();
        let total_uncles_count = snapshot
            .get_block_ext(&tip.hash())
            .unwrap()
            .total_uncles_count;
        assert_eq!(total_uncles_count, 5);

        let epoch = snapshot
            .next_epoch_ext(shared.consensus(), snapshot.epoch_ext(), &tip)
            .unwrap();

        // last_uncles_count 5
        // last_epoch_length 400
        // epoch_duration_target 14400
        // last_duration 7980

        // C_i+1,m = (o_ideal * (1 + o_i ) * L_ideal × C_i,m) / (o_i * (1 + o_ideal ) * L_i)
        // (1/40 * (1 + 5 / 400) * 14400 * 400) / (5 / 400 * ( 1+ 1/40) * 7980)
        // (405 * 14400 * 400) / (41 * 5 * 7980) = 1426
        // upper bound trigger
        assert_eq!(epoch.length(), 800, "epoch length {}", epoch.length());

        // orphan_rate_estimation = 1 / ( (1 + o_i ) * L_ideal * C_i,m / (o_i * L_i * C_i+1,m) − 1) = 133 / 9587
        // Diff_i+1 = (HPS_i · L_ideal) / (1 + orphan_rate_estimation ) * C_i+1,m
        // 50 * 14400 * 9587 / ((133 + 9587) * 800)
        assert_eq!(
            epoch.compact_target(),
            difficulty_to_compact(U256::from(887u64)), // 887
            "epoch compact_target {}",
            compact_to_difficulty(epoch.compact_target())
        );
    }

    let (_chain_controller, shared, _genesis, _last_epoch) =
        prepare_context_chain(consensus, 151, 20_000);
    {
        let snapshot = shared.snapshot();
        let tip = snapshot.tip_header().clone();
        let total_uncles_count = snapshot
            .get_block_ext(&tip.hash())
            .unwrap()
            .total_uncles_count;
        assert_eq!(total_uncles_count, 150);

        // last_uncles_count 150
        // last_epoch_length 400
        // epoch_duration_target 14400
        // last_duration 7980
        let epoch = snapshot
            .next_epoch_ext(shared.consensus(), snapshot.epoch_ext(), &tip)
            .unwrap();

        // C_i+1,m = (o_ideal * (1 + o_i ) * L_ideal × C_i,m) / (o_i * (1 + o_ideal ) * L_i)
        // ((150 + 400) * 14400 * 400) / ((40 + 1) * 150 * 7980) = 64
        // lower bound trigger
        assert_eq!(epoch.length(), 300, "epoch length {}", epoch.length());

        // orphan_rate_estimation  399 / 3121
        // (400 + 150) * 1000 / 7980  last_epoch_hash_rate 68
        // 68 * 14400 * 3121 / (3520 * 300 )
        assert_eq!(
            epoch.compact_target(),
            difficulty_to_compact(U256::from(2894u64)),
            "epoch difficulty {}",
            compact_to_difficulty(epoch.compact_target())
        );
    }
}
