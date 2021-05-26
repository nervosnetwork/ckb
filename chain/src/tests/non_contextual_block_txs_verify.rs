use crate::tests::util::{
    calculate_reward, create_always_success_out_point, create_always_success_tx, start_chain,
    MockStore,
};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_dao_utils::genesis_dao_data;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, BlockBuilder, BlockView, Capacity, EpochNumberWithFraction, HeaderView,
        TransactionBuilder, TransactionView,
    },
    packed::{CellDep, CellInput, CellOutputBuilder, OutPoint},
    utilities::DIFF_TWO,
};
use std::sync::Arc;

const TX_FEE: Capacity = capacity_bytes!(10);

#[allow(clippy::int_plus_one)]
pub(crate) fn create_cellbase(
    parent: &HeaderView,
    store: &MockStore,
    consensus: &Consensus,
) -> TransactionView {
    let (_, _, always_success_script) = always_success_cell();

    let number = parent.number() + 1;
    let capacity = calculate_reward(store, consensus, parent);
    let builder = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .witness(always_success_script.clone().into_witness());

    if (parent.number() + 1) <= consensus.finalization_delay_length() {
        builder.build()
    } else {
        builder
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity.pack())
                    .lock(always_success_script.clone())
                    .build(),
            )
            .output_data(Bytes::new().pack())
            .build()
    }
}

pub(crate) fn gen_block(
    parent_header: &HeaderView,
    transactions: Vec<TransactionView>,
    shared: &Shared,
    store: &MockStore,
) -> BlockView {
    let number = parent_header.number() + 1;
    let consensus = shared.consensus();
    let cellbase = create_cellbase(parent_header, store, consensus);
    let mut txs = vec![cellbase];
    txs.extend_from_slice(&transactions);

    let epoch = shared
        .consensus()
        .next_epoch_ext(&parent_header, &shared.store().as_data_provider())
        .unwrap()
        .epoch();

    let block = BlockBuilder::default()
        .parent_hash(parent_header.hash())
        .timestamp((parent_header.timestamp() + 20_000).pack())
        .number(number.pack())
        .compact_target(epoch.compact_target().pack())
        .epoch(epoch.number_with_fraction(number).pack())
        .transactions(txs)
        .build();

    store.insert_block(&block, consensus.genesis_epoch_ext());
    block
}

pub(crate) fn create_transaction(
    parent: &TransactionView,
    index: u32,
    missing_output_data: bool,
) -> TransactionView {
    let (_, _, always_success_script) = always_success_cell();
    let always_success_out_point = create_always_success_out_point();

    let input_cap: Capacity = parent
        .outputs()
        .get(0)
        .expect("get output index 0")
        .capacity()
        .unpack();

    let mut builder = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(input_cap.safe_sub(TX_FEE).unwrap().pack())
                .lock(always_success_script.clone())
                .build(),
        )
        .input(CellInput::new(OutPoint::new(parent.hash(), index), 0))
        .cell_dep(
            CellDep::new_builder()
                .out_point(always_success_out_point)
                .build(),
        );

    if !missing_output_data {
        builder = builder.output_data(Bytes::new().pack())
    }
    builder.build()
}

// Ensure block txs syntactic correctness checked before resolve
#[test]
fn non_contextual_block_txs_verify() {
    let (_, _, always_success_script) = always_success_cell();
    let always_success_tx = create_always_success_tx();
    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(5_000).pack())
                .lock(always_success_script.clone())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let dao = genesis_dao_data(vec![&always_success_tx, &issue_tx]).unwrap();

    let genesis_block = BlockBuilder::default()
        .transaction(always_success_tx)
        .transaction(issue_tx.clone())
        .compact_target(DIFF_TWO.pack())
        .dao(dao)
        .build();

    let consensus = ConsensusBuilder::default()
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .genesis_block(genesis_block)
        .build();

    let (chain_controller, shared, parent) = start_chain(Some(consensus));
    let mock_store = MockStore::new(&parent, shared.store());

    let tx0 = create_transaction(&issue_tx, 0, true);
    let tx1 = create_transaction(&tx0, 0, false);

    let block = gen_block(&parent, vec![tx0, tx1], &shared, &mock_store);

    let ret = chain_controller.process_block(Arc::new(block));
    assert!(ret.is_err());
    assert_eq!(
        format!("{}", ret.err().unwrap()),
        "Transaction(OutputsDataLengthMismatch: expected outputs data length (0) = outputs length (1))"
    );
}
