use crate::tests::util::{create_always_success_out_point, create_always_success_tx, start_chain};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_dao_utils::genesis_dao_data;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, BlockBuilder, Capacity, EpochNumberWithFraction, TransactionBuilder,
        TransactionView,
    },
    packed::{Block, CellDep, CellInput, CellOutput, CellOutputBuilder, OutPoint},
    utilities::DIFF_TWO,
};
use ckb_verification_traits::Switch;
use std::sync::Arc;

pub(crate) fn build_tx(
    parent: &TransactionView,
    inputs: &[u32],
    cell_deps: &[u32],
    outputs_len: usize,
    fee: Capacity,
) -> TransactionView {
    let (_, _, always_success_script) = always_success_cell();
    let always_success_out_point = create_always_success_out_point();

    let input_cap = inputs
        .iter()
        .map(|index| {
            let output = parent.output(*index as usize).expect("output index");
            let cap: Capacity = output.capacity().unpack();
            cap
        })
        .try_fold(Capacity::zero(), Capacity::safe_add)
        .unwrap();

    let per_output_capacity =
        Capacity::shannons((input_cap.safe_sub(fee).unwrap()).as_u64() / outputs_len as u64);

    TransactionBuilder::default()
        .outputs(
            (0..outputs_len)
                .map(|_| {
                    CellOutputBuilder::default()
                        .capacity(per_output_capacity.pack())
                        .lock(always_success_script.clone())
                        .build()
                })
                .collect::<Vec<CellOutput>>(),
        )
        .outputs_data((0..outputs_len).map(|_| Bytes::new().pack()))
        .cell_deps(cell_deps.iter().map(|index| {
            CellDep::new_builder()
                .out_point(OutPoint::new(parent.hash(), *index))
                .build()
        }))
        .inputs(
            inputs
                .iter()
                .map(|index| CellInput::new(OutPoint::new(parent.hash(), *index), 0)),
        )
        .cell_dep(
            CellDep::new_builder()
                .out_point(always_success_out_point)
                .build(),
        )
        .build()
}

#[test]
fn test_package_txs_with_deps() {
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

    let (chain_controller, shared, _parent) = start_chain(Some(consensus));

    let tx_pool = shared.tx_pool_controller();

    let tx1 = build_tx(&issue_tx, &[0], &[1], 2, Capacity::shannons(400));
    let tx2 = build_tx(&issue_tx, &[1], &[], 1, Capacity::shannons(10000));

    let mut block_template = tx_pool
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();

    // proposal txs
    {
        while (Into::<u64>::into(block_template.number)) != 1 {
            block_template = tx_pool
                .get_block_template(None, None, None)
                .unwrap()
                .unwrap()
        }

        let block: Block = block_template.clone().into();
        let block = block
            .as_advanced_builder()
            .proposals(vec![tx1.proposal_short_id(), tx2.proposal_short_id()])
            .build();
        chain_controller
            .internal_process_block(Arc::new(block), Switch::DISABLE_ALL)
            .unwrap();
    }

    // skip gap
    {
        while (Into::<u64>::into(block_template.number)) != 2 {
            block_template = tx_pool
                .get_block_template(None, None, None)
                .unwrap()
                .unwrap()
        }

        let block: Block = block_template.clone().into();
        let block = block.as_advanced_builder().build();
        chain_controller
            .internal_process_block(Arc::new(block), Switch::DISABLE_ALL)
            .unwrap();
    }

    // submit txs
    let ret1 = tx_pool.submit_local_tx(tx1).unwrap();
    assert!(ret1.is_ok(), "submit {:?}", ret1);
    let ret2 = tx_pool.submit_local_tx(tx2.clone()).unwrap();
    assert!(ret2.is_ok(), "submit {:?}", ret2);

    let mut tx_pool_info = tx_pool.get_tx_pool_info().unwrap();
    while tx_pool_info.proposed_size != 2 {
        tx_pool_info = tx_pool.get_tx_pool_info().unwrap()
    }

    // get block template with txs
    while (Into::<u64>::into(block_template.number)) != 3 {
        block_template = tx_pool
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap()
    }

    let block: Block = block_template.into();
    let block = block.as_advanced_builder().build();
    assert_eq!(block.transactions().len(), 2);
    assert_eq!(block.transactions()[1], tx2);
}
