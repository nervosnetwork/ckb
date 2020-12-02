use crate::tests::util::{
    create_load_input_one_byte_cell_tx, create_load_input_one_byte_out_point, start_chain,
};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_dao_utils::genesis_dao_data;
use ckb_test_chain_utils::load_input_one_byte_cell;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, BlockBuilder, Capacity, EpochNumberWithFraction, TransactionBuilder,
        TransactionView,
    },
    packed::{CellDep, CellInput, CellOutputBuilder, OutPoint},
    utilities::DIFF_TWO,
};

const TX_FEE: Capacity = capacity_bytes!(10);

pub(crate) fn create_load_input_one_byte_transaction(
    parent: &TransactionView,
    index: u32,
) -> TransactionView {
    let (_, _, load_input_one_byte_script) = load_input_one_byte_cell();
    let load_input_one_byte_out_point = create_load_input_one_byte_out_point();

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
                .lock(load_input_one_byte_script.clone())
                .build(),
        )
        .output_data(Bytes::from(b"1".to_vec()).pack())
        .input(CellInput::new(OutPoint::new(parent.hash(), index), 0))
        .cell_dep(
            CellDep::new_builder()
                .out_point(load_input_one_byte_out_point)
                .build(),
        )
        .build()
}

// Permit tx-pool accept tx which calls syscall load_cell_data from input
#[test]
fn test_load_input_one_byte_cell() {
    let (_, _, load_input_one_byte_script) = load_input_one_byte_cell();
    let load_input_one_byte_cell_tx = create_load_input_one_byte_cell_tx();

    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(5_000).pack())
                .lock(load_input_one_byte_script.clone())
                .build(),
        )
        .output_data(Bytes::from(b"1".to_vec()).pack())
        .build();

    let dao = genesis_dao_data(vec![&load_input_one_byte_cell_tx, &issue_tx]).unwrap();

    let genesis_block = BlockBuilder::default()
        .transaction(load_input_one_byte_cell_tx)
        .transaction(issue_tx.clone())
        .compact_target(DIFF_TWO.pack())
        .dao(dao)
        .build();

    let consensus = ConsensusBuilder::default()
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .genesis_block(genesis_block)
        .build();

    let (_chain_controller, shared, _parent) = start_chain(Some(consensus));

    let tx0 = create_load_input_one_byte_transaction(&issue_tx, 0);
    let tx1 = create_load_input_one_byte_transaction(&tx0, 0);

    let tx_pool = shared.tx_pool_controller();
    let ret = tx_pool.submit_txs(vec![tx0]).unwrap();
    assert!(ret.is_ok());

    // Permit tx-pool accept tx which calls syscall load_cell_data from input
    let ret = tx_pool.submit_txs(vec![tx1]).unwrap();
    assert!(ret.is_ok());
}
