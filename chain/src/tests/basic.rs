use crate::chain::ChainController;
use crate::tests::util::{
    create_always_success_tx, create_cellbase, create_multi_outputs_transaction,
    create_transaction, create_transaction_with_out_point, dao_data, start_chain, MockChain,
    MockStore,
};
use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::genesis_dao_data;
use ckb_shared::error::SharedError;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes,
        cell::{CellMeta, CellProvider, CellStatus, UnresolvableError},
        BlockBuilder, BlockView, Capacity, EpochExt, HeaderView, TransactionBuilder,
        TransactionInfo,
    },
    packed::{CellInput, CellOutputBuilder, OutPoint, Script},
    H256, U256,
};
use std::sync::Arc;

#[test]
fn repeat_process_block() {
    let (chain_controller, shared, parent) = start_chain(None);
    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent.clone(), shared.consensus());
    chain.gen_empty_block(100u64, &mock_store);
    let block = Arc::new(chain.blocks().last().unwrap().clone());

    assert!(chain_controller
        .process_block(Arc::clone(&block), true)
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
        .process_block(Arc::clone(&block), true)
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

    let mut root_hash = tx.hash().to_owned();

    let genesis_tx_hash = root_hash.clone();

    let dao = genesis_dao_data(&tx).unwrap();

    let genesis_block = BlockBuilder::default()
        .transaction(tx)
        .transaction(create_always_success_tx())
        .difficulty(U256::from(1000u64).pack())
        .dao(dao)
        .build();

    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (chain_controller, shared, parent) = start_chain(Some(consensus));

    let end = 21;

    let mock_store = MockStore::new(&parent, shared.store());

    let mut chain = MockChain::new(parent.clone(), shared.consensus());

    for i in 1..end {
        let tx = create_transaction(&root_hash, i as u8);
        root_hash = tx.hash().to_owned();

        // commit txs in block
        chain.gen_block_with_commit_txs(vec![tx], &mock_store, false);
    }

    for block in &chain.blocks()[0..10] {
        assert!(chain_controller
            .process_block(Arc::new(block.clone()), false)
            .is_ok());
    }

    assert_eq!(
        shared
            .snapshot()
            .cell(&OutPoint::new(genesis_tx_hash.unpack(), 0), false),
        CellStatus::Dead
    );
}

#[test]
fn test_transaction_spend_in_same_block() {
    let (chain_controller, shared, parent) = start_chain(None);
    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent.clone(), shared.consensus());
    chain.gen_empty_block(100u64, &mock_store);

    let last_cell_base = &chain.tip().transactions()[0];
    let last_cell_base_hash = last_cell_base.hash().to_owned();
    let tx1 = create_multi_outputs_transaction(&last_cell_base, vec![0], 2, vec![1]);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![2]);
    let tx2_hash = tx2.hash().to_owned();
    let tx2_output = tx2.outputs().get(0).expect("outputs index 0").clone();
    let tx2_output_data = tx2
        .outputs_data()
        .get(0)
        .expect("outputs_data index 0")
        .clone();

    let txs = vec![tx1, tx2];

    for hash in &[&last_cell_base_hash, &tx1_hash, &tx2_hash] {
        assert_eq!(
            shared
                .snapshot()
                .cell(&OutPoint::new(hash.unpack(), 0), false),
            CellStatus::Unknown
        );
    }

    // proposal txs
    chain.gen_block_with_proposal_txs(txs.clone(), &mock_store);
    // empty block
    chain.gen_empty_block(100, &mock_store);
    // commit txs in block
    chain.gen_block_with_commit_txs(txs.clone(), &mock_store, false);
    let (parent_hash4, parent_number4) = {
        chain
            .blocks()
            .last()
            .map(|block| (block.header().hash().clone(), block.header().number()))
            .unwrap()
    };

    for block in chain.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }

    // assert last_cell_base_hash is full dead
    assert_eq!(
        shared
            .snapshot()
            .cell(&OutPoint::new(last_cell_base_hash.unpack(), 0), false),
        CellStatus::Unknown
    );

    assert_eq!(
        shared
            .snapshot()
            .cell(&OutPoint::new(tx1_hash.unpack(), 0), false),
        CellStatus::Dead
    );

    assert_eq!(
        shared
            .snapshot()
            .cell(&OutPoint::new(tx2_hash.unpack(), 0), false),
        CellStatus::live_cell(CellMeta {
            cell_output: tx2_output,
            data_bytes: tx2_output_data.len() as u64,
            out_point: OutPoint::new(tx2_hash.unpack(), 0),
            transaction_info: Some(TransactionInfo::new(
                parent_number4,
                0,
                parent_hash4.unpack(),
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
    let mut chain = MockChain::new(parent.clone(), shared.consensus());
    chain.gen_empty_block(100u64, &mock_store);

    let last_cell_base = &chain.tip().transactions()[0];
    let tx1 = create_transaction(&last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_transaction(&tx1_hash, 2);
    let tx3 = create_transaction(&tx1_hash, 3);
    let txs = vec![tx1, tx2, tx3];

    // proposal txs
    chain.gen_block_with_proposal_txs(txs.clone(), &mock_store);
    // empty block
    chain.gen_empty_block(100, &mock_store);
    // commit txs in block
    chain.gen_block_with_commit_txs(txs.clone(), &mock_store, true);

    for block in chain.blocks().iter().take(3) {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }
    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Dead(OutPoint::new(
            tx1_hash.unpack(),
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
    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent.clone(), shared.consensus());
    chain.gen_empty_block(100u64, &mock_store);

    let last_cell_base = &chain.tip().transactions()[0];
    let tx1 = create_multi_outputs_transaction(&last_cell_base, vec![0], 2, vec![1]);
    let tx1_hash = tx1.hash();
    let tx2 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![1]);
    let tx3 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![2]);
    // proposal txs
    chain.gen_block_with_proposal_txs(vec![tx1.clone(), tx2.clone(), tx3.clone()], &mock_store);

    // empty N+1 block
    chain.gen_empty_block(100, &mock_store);

    // commit tx1 and tx2 in N+2 block
    chain.gen_block_with_commit_txs(vec![tx1.clone(), tx2.clone()], &mock_store, false);

    // commit tx3 in N+3 block
    chain.gen_block_with_commit_txs(vec![tx3.clone()], &mock_store, false);

    for block in chain.blocks().iter().take(4) {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }
    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Dead(OutPoint::new(
            tx1_hash.unpack(),
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
    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent.clone(), shared.consensus());
    chain.gen_empty_block(100u64, &mock_store);

    let last_cell_base = &chain.tip().transactions()[0];
    let tx1 = create_transaction(&last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_transaction(&tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new(tx1_hash.unpack(), 1), 3);
    let txs = vec![tx1, tx2, tx3];
    // proposal txs
    chain.gen_block_with_proposal_txs(txs.clone(), &mock_store);
    // empty N+1 block
    chain.gen_empty_block(100, &mock_store);
    // commit txs in N+2 block
    chain.gen_block_with_commit_txs(txs.clone(), &mock_store, true);

    for block in chain.blocks().iter().take(3) {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }
    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Unknown(vec![OutPoint::new(
            tx1_hash.unpack(),
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
    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent.clone(), shared.consensus());
    chain.gen_empty_block(100u64, &mock_store);

    let last_cell_base = &chain.tip().transactions()[0];
    let tx1 = create_transaction(&last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(&tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new(tx1_hash.unpack(), 1), 3);
    // proposal txs
    chain.gen_block_with_proposal_txs(vec![tx1.clone(), tx2.clone(), tx3.clone()], &mock_store);
    // empty N+1 block
    chain.gen_empty_block(100, &mock_store);
    // commit tx1 and tx2 in N+2 block
    chain.gen_block_with_commit_txs(vec![tx1.clone(), tx2.clone()], &mock_store, false);
    // commit tx3 in N+3 block
    chain.gen_block_with_commit_txs(vec![tx3.clone()], &mock_store, true);

    for block in chain.blocks().iter().take(4) {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }

    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Unknown(vec![OutPoint::new(
            tx1_hash.unpack(),
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
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(100_000_000).pack())
                .lock(Script::default())
                .build();
            100
        ])
        .outputs_data(vec![Bytes::new(); 100].pack())
        .build();

    let root_hash = tx.hash().to_owned();

    let genesis_block = BlockBuilder::default()
        .transaction(tx)
        .difficulty(U256::from(1000u64).pack())
        .build();

    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (_chain_controller, shared, _parent) = start_chain(Some(consensus));

    let out_point = OutPoint::new(root_hash.unpack(), 0);
    let state = shared.snapshot().cell(&out_point, false);
    assert!(state.is_live());
}

#[test]
fn test_chain_fork_by_total_difficulty() {
    let (chain_controller, shared, parent) = start_chain(None);
    let final_number = 20;

    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain1 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain2 = MockChain::new(parent.clone(), shared.consensus());

    // 100 * 20 = 2000
    for _ in 0..final_number {
        chain1.gen_empty_block_with_difficulty(100u64, &mock_store);
    }

    // 99 * 10 + 110 * 10 = 2090
    for i in 0..final_number {
        let j = if i > 10 { 110 } else { 99 };
        chain2.gen_empty_block_with_difficulty(j, &mock_store);
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

    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain1 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain2 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain3 = MockChain::new(parent.clone(), shared.consensus());

    // 100 * 20 = 2000
    for _ in 0..final_number {
        chain1.gen_empty_block_with_difficulty(100u64, &mock_store);
    }

    // 50 * 40 = 2000
    for _ in 0..(final_number * 2) {
        chain2.gen_empty_block_with_difficulty(50u64, &mock_store);
    }

    // 20 * 100 = 2000
    for _ in 0..(final_number * 5) {
        chain3.gen_empty_block_with_difficulty(20u64, &mock_store);
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

    let hash1: H256 = chain1.tip_header().hash().unpack();
    let hash2: H256 = chain2.tip_header().hash().unpack();
    let hash3: H256 = chain3.tip_header().hash().unpack();

    let tips = vec![hash1.clone(), hash2.clone(), hash3.clone()];
    let v = tips.iter().min().unwrap();

    let best = match v {
        hash if hash == &hash1 => chain1,
        hash if hash == &hash2 => chain2,
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

    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain1 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain2 = MockChain::new(parent.clone(), shared.consensus());

    for _ in 1..final_number {
        chain1.gen_empty_block(100u64, &mock_store);
    }

    for _ in 1..final_number {
        chain2.gen_empty_block(90u64, &mock_store);
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
        chain1.blocks()[9].header(),
        shared
            .store()
            .get_ancestor(&chain1.tip_header().hash(), 10)
            .unwrap()
    );

    assert_eq!(
        chain2.blocks()[9].header(),
        shared
            .store()
            .get_ancestor(&chain2.tip_header().hash(), 10)
            .unwrap()
    );
}

fn prepare_context_chain(
    consensus: Consensus,
    orphan_count: u64,
    timestep: u64,
) -> (ChainController, Shared, HeaderView, EpochExt) {
    let epoch = consensus.genesis_epoch_ext.clone();
    let (chain_controller, shared, genesis) = start_chain(Some(consensus.clone()));
    let final_number = shared.consensus().genesis_epoch_ext().length();

    let mut chain1: Vec<BlockView> = Vec::new();
    let mut chain2: Vec<BlockView> = Vec::new();

    let mut parent = genesis.clone();
    let mut last_epoch = epoch.clone();

    let mock_store = MockStore::new(&parent, shared.store());

    for _ in 1..final_number - 1 {
        let epoch = shared
            .next_epoch_ext(&last_epoch, &parent)
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
            .difficulty(epoch.difficulty().pack())
            .transactions(transactions)
            .dao(dao)
            .build();

        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .expect("process block ok");
        chain1.push(new_block.clone());
        mock_store.insert_block(&new_block, &epoch);
        parent = new_block.header().clone();
        last_epoch = epoch;
    }

    parent = genesis.clone();
    let mut last_epoch = epoch.clone();
    for i in 1..final_number {
        let epoch = shared
            .next_epoch_ext(&last_epoch, &parent)
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
            .difficulty(epoch.difficulty().pack())
            .transactions(transactions)
            .dao(dao)
            .build();

        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
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
    let dao = genesis_dao_data(&cellbase).unwrap();
    let genesis_block = BlockBuilder::default()
        .difficulty(U256::from(1000u64).pack())
        .transaction(cellbase)
        .dao(dao)
        .build();

    // last_difficulty 1000
    // last_uncles_count 25
    // last_epoch_length 400
    // last_duration 7980
    let mut consensus = Consensus::default().set_genesis_block(genesis_block.clone());
    consensus.genesis_epoch_ext.set_length(400);
    consensus
        .genesis_epoch_ext
        .set_previous_epoch_hash_rate(U256::from(10u64));
    let (_chain_controller, shared, _genesis, _last_epoch) =
        prepare_context_chain(consensus.clone(), 26, 20_000);

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

    let mut consensus = Consensus::default().set_genesis_block(genesis_block);
    consensus.genesis_epoch_ext.set_length(400);
    consensus
        .genesis_epoch_ext
        .set_previous_epoch_hash_rate(U256::from(200u64));
    let (_chain_controller, shared, _genesis, _last_epoch) =
        prepare_context_chain(consensus.clone(), 26, 20_000);

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
    let dao = genesis_dao_data(&cellbase).unwrap();

    let genesis_block = BlockBuilder::default()
        .difficulty(U256::from(1000u64).pack())
        .transaction(cellbase)
        .dao(dao)
        .build();

    let mut consensus = Consensus::default().set_genesis_block(genesis_block);
    consensus.genesis_epoch_ext.set_length(400);

    // last_difficulty 1000
    // last_epoch_length 400
    // epoch_duration_target 14400
    // orphan_rate_target 1/20
    // last_duration 798000
    // last_uncles_count 150
    let (_chain_controller, shared, _genesis, _last_epoch) =
        prepare_context_chain(consensus.clone(), 151, 2_000_000);
    {
        let snapshot = shared.snapshot();
        let tip = snapshot.tip_header().clone();
        let total_uncles_count = shared
            .store()
            .get_block_ext(&tip.hash())
            .unwrap()
            .total_uncles_count;
        assert_eq!(total_uncles_count, 150);

        let epoch = snapshot
            .next_epoch_ext(shared.consensus(), snapshot.epoch_ext(), &tip)
            .unwrap();

        assert_eq!(epoch.length(), 480, "epoch length {}", epoch.length());

        // orphan_rate_estimation (22/399 - 1) overflow
        // max((400 + 150) * 1000 / 798000, 1)  last_epoch_hash_rate 1
        // 14400 * 20 / (21 * 480)
        assert_eq!(
            epoch.difficulty(),
            &U256::from(28u64),
            "epoch difficulty {}",
            epoch.difficulty()
        );
    }
}

#[test]
fn test_next_epoch_ext() {
    let cellbase = TransactionBuilder::default()
        .witness(Script::default().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .build();
    let dao = genesis_dao_data(&cellbase).unwrap();
    let genesis_block = BlockBuilder::default()
        .difficulty(U256::from(1000u64).pack())
        .transaction(cellbase)
        .dao(dao)
        .build();

    let mut consensus = Consensus::default().set_genesis_block(genesis_block);
    consensus.genesis_epoch_ext.set_length(400);

    // last_difficulty 1000
    // last_epoch_length 400
    // epoch_duration_target 14400
    // orphan_rate_target 1/20
    // last_duration 7980
    let (_chain_controller, shared, _genesis, _last_epoch) =
        prepare_context_chain(consensus.clone(), 26, 20_000);
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

        // last_uncles_count 25
        // HPS  = dampen(last_difficulty * (last_epoch_length + last_uncles_count) / last_duration)
        assert_eq!(
            epoch.previous_epoch_hash_rate(),
            &U256::from(53u64),
            "previous_epoch_hash_rate {}",
            epoch.previous_epoch_hash_rate()
        );

        // C_i+1,m = (o_ideal * (1 + o_i ) * L_ideal × C_i,m) / (o_i * (1 + o_ideal ) * L_i)
        // (1/20 * (1+ 25/400) * 14400 * 400) / (25 / 400 * ( 1+ 1/20) * 7980)
        // (425 * 14400 * 400) / (25 * 21 * 7980)
        assert_eq!(epoch.length(), 584, "epoch length {}", epoch.length());

        // None of the edge cases is triggered
        // Diff_i+1 = (HPS_i · L_ideal) / (1 + 0_i+1 ) * C_i+1,m
        // (53 * 14400) / ((1 + 1/20) * 584)
        // (20 * 53 * 14400) / (21 * 584)
        assert_eq!(
            epoch.difficulty(),
            &U256::from(1244u64),
            "epoch difficulty {}",
            epoch.difficulty()
        );

        let consensus = shared.consensus();
        let epoch_reward = consensus.epoch_reward();
        let block_reward = Capacity::shannons(epoch_reward.as_u64() / epoch.length());
        let block_reward1 = block_reward.safe_add(Capacity::one()).unwrap();
        let bound = 400 + epoch.remainder_reward().as_u64();

        // block_reward 428082191780
        // remainder_reward 960
        assert_eq!(
            epoch.block_reward(400).unwrap(),
            block_reward1,
            "block_reward {:?}, remainder_reward{:?}",
            block_reward,
            epoch.remainder_reward()
        );

        assert_eq!(epoch.block_reward(bound - 1).unwrap(), block_reward1);
        assert_eq!(epoch.block_reward(bound).unwrap(), block_reward);
    }

    let (_chain_controller, shared, _genesis, _last_epoch) =
        prepare_context_chain(consensus.clone(), 11, 20_000);
    {
        let snapshot = shared.snapshot();
        let tip = snapshot.tip_header().clone();
        let total_uncles_count = snapshot
            .get_block_ext(&tip.hash())
            .unwrap()
            .total_uncles_count;
        assert_eq!(total_uncles_count, 10);

        let epoch = snapshot
            .next_epoch_ext(shared.consensus(), snapshot.epoch_ext(), &tip)
            .unwrap();

        // last_uncles_count 10
        // last_epoch_length 400
        // epoch_duration_target 14400
        // last_duration 7980

        // C_i+1,m = (o_ideal * (1 + o_i ) * L_ideal × C_i,m) / (o_i * (1 + o_ideal ) * L_i)
        // (1/20 * (1+ 10/400) * 14400 * 400) / (10 / 400 * ( 1+ 1/20) * 7980)
        // (410 * 14400 * 400) / (21 * 10 * 7980) = 1409
        // upper bound trigger
        assert_eq!(epoch.length(), 800, "epoch length {}", epoch.length());

        // orphan_rate_estimation = 1 / ( (1 + o_i ) * L_ideal * C_i,m / (o_i * L_i * C_i+1,m) − 1) = 133 / 4787
        // Diff_i+1 = (HPS_i · L_ideal) / (1 + orphan_rate_estimation ) * C_i+1,m
        // 51 * 14400 * 4787 / ((133 + 4787) * 800)
        assert_eq!(
            epoch.difficulty(),
            &U256::from(893u64),
            "epoch difficulty {}",
            epoch.difficulty()
        );
    }

    let (_chain_controller, shared, _genesis, _last_epoch) =
        prepare_context_chain(consensus.clone(), 151, 20_000);
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
        // ((150 + 400) * 14400 * 400) / ((20 + 1) * 150 * 7980) = 126
        // lower bound trigger
        assert_eq!(epoch.length(), 480, "epoch length {}", epoch.length());

        // orphan_rate_estimation  399 / 1810
        // (400 + 150) * 1000 / 7980  last_epoch_hash_rate 68
        // 1801 * 68 * 14400 / (2200 * 480)
        assert_eq!(
            epoch.difficulty(),
            &U256::from(1670u64),
            "epoch difficulty {}",
            epoch.difficulty()
        );
    }
}
