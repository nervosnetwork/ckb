use crate::tests::util::{
    calculate_reward, create_always_success_out_point, create_always_success_tx, dao_data,
    start_chain, MockStore,
};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_dao_utils::genesis_dao_data;
use ckb_reward_calculator::RewardCalculator;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, BlockBuilder, BlockView, Capacity, EpochNumberWithFraction, HeaderView,
        ScriptHashType, TransactionBuilder, TransactionView, UncleBlockView,
    },
    packed::{
        self, CellDep, CellInput, CellOutputBuilder, OutPoint, ProposalShortId, Script,
        ScriptBuilder,
    },
    utilities::DIFF_TWO,
};
use std::sync::Arc;

const TX_FEE: Capacity = capacity_bytes!(10);

#[allow(clippy::int_plus_one)]
pub(crate) fn create_cellbase(
    parent: &HeaderView,
    miner_lock: Script,
    reward_lock: Script,
    reward: Option<Capacity>,
    store: &MockStore,
    consensus: &Consensus,
) -> TransactionView {
    let number = parent.number() + 1;
    let capacity = calculate_reward(store, consensus, parent);
    let builder = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .witness(miner_lock.into_witness());

    if (parent.number() + 1) <= consensus.finalization_delay_length() {
        builder.build()
    } else {
        builder
            .output(
                CellOutputBuilder::default()
                    .capacity(reward.unwrap_or(capacity).pack())
                    .lock(reward_lock)
                    .build(),
            )
            .output_data(Bytes::new().pack())
            .build()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn gen_block(
    parent_header: &HeaderView,
    transactions: Vec<TransactionView>,
    proposals: Vec<ProposalShortId>,
    uncles: Vec<UncleBlockView>,
    miner_lock: Script,
    reward_lock: Script,
    reward: Option<Capacity>,
    shared: &Shared,
    store: &MockStore,
) -> BlockView {
    let number = parent_header.number() + 1;
    let consensus = shared.consensus();
    let cellbase = create_cellbase(
        parent_header,
        miner_lock,
        reward_lock,
        reward,
        store,
        consensus,
    );
    let mut txs = vec![cellbase];
    txs.extend_from_slice(&transactions);

    let dao = dao_data(consensus, parent_header, &txs, store, false);

    let epoch = shared
        .consensus()
        .next_epoch_ext(parent_header, &shared.store().borrow_as_data_loader())
        .unwrap()
        .epoch();

    let block = BlockBuilder::default()
        .parent_hash(parent_header.hash())
        .timestamp((parent_header.timestamp() + 20_000).pack())
        .number(number.pack())
        .compact_target(epoch.compact_target().pack())
        .epoch(epoch.number_with_fraction(number).pack())
        .dao(dao)
        .transactions(txs)
        .uncles(uncles)
        .proposals(proposals)
        .build();

    store.insert_block(&block, consensus.genesis_epoch_ext());

    block
}

pub(crate) fn create_transaction(parent: &TransactionView, index: u32) -> TransactionView {
    let (_, _, always_success_script) = always_success_cell();
    let always_success_out_point = create_always_success_out_point();

    let input_cap: Capacity = parent
        .outputs()
        .get(0)
        .expect("get output index 0")
        .capacity()
        .unpack();

    TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(input_cap.safe_sub(TX_FEE).unwrap().pack())
                .lock(always_success_script.clone())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .input(CellInput::new(OutPoint::new(parent.hash(), index), 0))
        .cell_dep(
            CellDep::new_builder()
                .out_point(always_success_out_point)
                .build(),
        )
        .build()
}

#[test]
fn finalize_reward() {
    let (_, _, always_success_script) = always_success_cell();
    let always_success_tx = create_always_success_tx();
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(5_000).pack())
                .lock(always_success_script.clone())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let dao = genesis_dao_data(vec![&always_success_tx, &tx]).unwrap();

    let genesis_block = BlockBuilder::default()
        .transaction(always_success_tx)
        .transaction(tx.clone())
        .compact_target(DIFF_TWO.pack())
        .dao(dao)
        .build();

    let consensus = ConsensusBuilder::default()
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .genesis_block(genesis_block)
        .build();

    let (chain_controller, shared, mut parent) = start_chain(Some(consensus));

    let mock_store = MockStore::new(&parent, shared.store());

    let mut txs = Vec::with_capacity(16);
    let mut tx_parent = tx;
    for _i in 0..16 {
        tx_parent = create_transaction(&tx_parent, 0);
        txs.push(tx_parent.clone());
    }

    let ids: Vec<_> = txs.iter().map(TransactionView::proposal_short_id).collect();
    let mut blocks = Vec::with_capacity(24);
    let bob_args: packed::Bytes = Bytes::from(b"b0b".to_vec()).pack();

    let bob = ScriptBuilder::default()
        .args(bob_args)
        .code_hash(always_success_script.code_hash())
        .hash_type(ScriptHashType::Data.into())
        .build();

    let alice_args: packed::Bytes = Bytes::from(b"a11ce".to_vec()).pack();
    let alice = ScriptBuilder::default()
        .args(alice_args)
        .code_hash(always_success_script.code_hash())
        .hash_type(ScriptHashType::Data.into())
        .build();

    for i in 1..23 {
        let proposals = if i == 12 {
            ids.iter().take(8).cloned().collect()
        } else if i == 13 {
            ids.clone()
        } else {
            vec![]
        };

        let miner_lock = if i == 12 {
            bob.clone()
        } else if i == 13 {
            alice.clone()
        } else {
            always_success_script.clone()
        };

        let block_txs = if i == 22 {
            txs.iter().take(12).cloned().collect()
        } else {
            vec![]
        };

        let block = gen_block(
            &parent,
            block_txs,
            proposals,
            vec![],
            miner_lock,
            always_success_script.clone(),
            None,
            &shared,
            &mock_store,
        );

        parent = block.header().clone();

        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
        blocks.push(block);
    }

    let (target, reward) = RewardCalculator::new(shared.consensus(), shared.snapshot().as_ref())
        .block_reward_to_finalize(&blocks[21].header())
        .unwrap();
    assert_eq!(target, bob);

    // bob proposed 8 txs in 12, committed in 22
    // get all proposal reward
    let block_reward = calculate_reward(&mock_store, shared.consensus(), &parent);
    let bob_reward = TX_FEE
        .safe_mul_ratio(shared.consensus().proposer_reward_ratio())
        .unwrap()
        .safe_mul(8u8) // 8 txs
        .unwrap()
        .safe_add(block_reward)
        .unwrap();
    assert_eq!(reward.total, bob_reward,);

    let block = gen_block(
        &parent,
        txs.iter().skip(12).cloned().collect(),
        vec![],
        vec![],
        always_success_script.clone(),
        target,
        Some(bob_reward),
        &shared,
        &mock_store,
    );

    parent = block.header();

    chain_controller
        .process_block(Arc::new(block.clone()))
        .expect("process block ok");

    let (target, reward) = RewardCalculator::new(shared.consensus(), shared.snapshot().as_ref())
        .block_reward_to_finalize(&block.header())
        .unwrap();
    assert_eq!(target, alice);

    // alice proposed 16 txs in block 13, committed in 22, 23
    // but bob proposed 8 txs earlier
    // get 8 proposal reward
    let block_reward = calculate_reward(&mock_store, shared.consensus(), &parent);
    let alice_reward = TX_FEE
        .safe_mul_ratio(shared.consensus().proposer_reward_ratio())
        .unwrap()
        .safe_mul(8u8)
        .unwrap()
        .safe_add(block_reward)
        .unwrap();
    assert_eq!(reward.total, alice_reward);

    let block = gen_block(
        &parent,
        vec![],
        vec![],
        vec![],
        always_success_script.clone(),
        target,
        Some(alice_reward),
        &shared,
        &mock_store,
    );

    chain_controller
        .process_block(Arc::new(block))
        .expect("process block ok");
}
