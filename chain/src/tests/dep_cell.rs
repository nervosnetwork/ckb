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
    inputs: (&TransactionView, &[u32]),
    cell_deps: (&TransactionView, &[u32]),
    outputs_len: usize,
    fee: Capacity,
) -> TransactionView {
    let (_, _, always_success_script) = always_success_cell();
    let always_success_out_point = create_always_success_out_point();

    let input_cap = inputs
        .1
        .iter()
        .map(|index| {
            let output = inputs.0.output(*index as usize).expect("output index");
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
        .cell_deps(cell_deps.1.iter().map(|index| {
            CellDep::new_builder()
                .out_point(OutPoint::new(cell_deps.0.hash(), *index))
                .build()
        }))
        .inputs(
            inputs
                .1
                .iter()
                .map(|index| CellInput::new(OutPoint::new(inputs.0.hash(), *index), 0)),
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

    let tx1 = build_tx(
        (&issue_tx, &[0]),
        (&issue_tx, &[1]),
        2,
        Capacity::shannons(400),
    );
    let tx2 = build_tx(
        (&issue_tx, &[1]),
        (&issue_tx, &[]),
        2,
        Capacity::shannons(10000),
    );
    let tx3 = build_tx((&tx2, &[0]), (&tx2, &[1]), 1, Capacity::shannons(400));
    let tx4 = build_tx((&tx2, &[1]), (&tx2, &[]), 2, Capacity::shannons(10000));

    let txs = vec![tx1, tx2, tx3, tx4];

    let mut block_template = shared
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();

    // proposal txs
    {
        while (Into::<u64>::into(block_template.number)) != 1 {
            block_template = shared
                .get_block_template(None, None, None)
                .unwrap()
                .unwrap()
        }

        let block: Block = block_template.clone().into();
        let block = block
            .as_advanced_builder()
            .proposals(
                txs.iter()
                    .map(|tx| tx.proposal_short_id())
                    .collect::<Vec<_>>(),
            )
            .build();
        chain_controller
            .internal_process_block(Arc::new(block), Switch::DISABLE_ALL)
            .unwrap();
    }

    // skip gap
    {
        while (Into::<u64>::into(block_template.number)) != 2 {
            block_template = shared
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
    for tx in &txs {
        let ret = tx_pool.submit_local_tx(tx.clone()).unwrap();
        assert!(ret.is_ok(), "submit {} {:?}", tx.proposal_short_id(), ret);
    }

    let mut tx_pool_info = tx_pool.get_tx_pool_info().unwrap();
    while tx_pool_info.proposed_size != txs.len() {
        tx_pool_info = tx_pool.get_tx_pool_info().unwrap()
    }

    // get block template with txs
    while !(Into::<u64>::into(block_template.number) == 3 && block_template.transactions.len() == 4)
    {
        block_template = shared
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap()
    }

    let block: Block = block_template.into();
    let block = block.as_advanced_builder().build();

    for (index, tx) in block.transactions().iter().skip(1).enumerate() {
        assert_eq!(tx.proposal_short_id(), txs[index].proposal_short_id());
    }
}

#[test]
fn test_package_txs_with_deps_unstable_sort() {
    let (_, _, always_success_script) = always_success_cell();
    let always_success_tx = create_always_success_tx();
    // 3 output
    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(5_000).pack())
                .lock(always_success_script.clone())
                .build(),
        )
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(5_000).pack())
                .lock(always_success_script.clone())
                .build(),
        )
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(5_000).pack())
                .lock(always_success_script.clone())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .output_data(Bytes::new().pack())
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

    let tx1 = build_tx(
        (&issue_tx, &[1]),
        (&issue_tx, &[0]),
        2,
        Capacity::shannons(400),
    );
    let tx2 = build_tx(
        (&issue_tx, &[2]),
        (&issue_tx, &[0]),
        2,
        Capacity::shannons(400),
    );
    let tx3 = build_tx(
        (&issue_tx, &[0]),
        (&issue_tx, &[]),
        3,
        Capacity::shannons(10000),
    );
    let tx4 = build_tx((&tx3, &[1]), (&tx3, &[0]), 2, Capacity::shannons(400));
    let tx5 = build_tx((&tx3, &[2]), (&tx3, &[0]), 2, Capacity::shannons(400));
    let tx6 = build_tx((&tx3, &[0]), (&tx3, &[]), 2, Capacity::shannons(10000));

    let txs = vec![tx1, tx2, tx3.clone(), tx4, tx5, tx6.clone()];

    let mut block_template = shared
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();

    // proposal txs
    {
        while Into::<u64>::into(block_template.number) != 1 {
            block_template = shared
                .get_block_template(None, None, None)
                .unwrap()
                .unwrap()
        }

        let block: Block = block_template.clone().into();
        let block = block
            .as_advanced_builder()
            .proposals(
                txs.iter()
                    .map(|tx| tx.proposal_short_id())
                    .collect::<Vec<_>>(),
            )
            .build();
        chain_controller
            .internal_process_block(Arc::new(block), Switch::DISABLE_ALL)
            .unwrap();
    }

    // skip gap
    {
        while Into::<u64>::into(block_template.number) != 2 {
            block_template = shared
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
    for tx in &txs {
        let ret = tx_pool.submit_local_tx(tx.clone()).unwrap();
        assert!(ret.is_ok(), "submit {} {:?}", tx.proposal_short_id(), ret);
    }

    let mut tx_pool_info = tx_pool.get_tx_pool_info().unwrap();
    while tx_pool_info.proposed_size != txs.len() {
        tx_pool_info = tx_pool.get_tx_pool_info().unwrap()
    }

    // get block template with txs
    while !(Into::<u64>::into(block_template.number) == 3 && block_template.transactions.len() == 6)
    {
        block_template = shared
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap()
    }

    let block: Block = block_template.into();
    let block = block.as_advanced_builder().build();

    let in_blocks = block.transactions();
    assert_eq!(tx3.proposal_short_id(), in_blocks[3].proposal_short_id());
    assert_eq!(tx6.proposal_short_id(), in_blocks[6].proposal_short_id());
}

#[test]
fn test_package_txs_with_deps2() {
    let (_, _, always_success_script) = always_success_cell();
    let always_success_tx = create_always_success_tx();
    // 3 output
    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(5_000).pack())
                .lock(always_success_script.clone())
                .build(),
        )
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(5_000).pack())
                .lock(always_success_script.clone())
                .build(),
        )
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(5_000).pack())
                .lock(always_success_script.clone())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .output_data(Bytes::new().pack())
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

    let tx1 = build_tx(
        (&issue_tx, &[1]),
        (&issue_tx, &[0]),
        2,
        Capacity::shannons(400),
    );
    let tx2 = build_tx((&tx1, &[0]), (&issue_tx, &[0]), 2, Capacity::shannons(400));
    let tx3 = build_tx(
        (&issue_tx, &[0]),
        (&issue_tx, &[]),
        3,
        Capacity::shannons(10000),
    );
    let tx4 = build_tx((&tx3, &[1]), (&tx3, &[0]), 2, Capacity::shannons(400));
    let tx5 = build_tx((&tx4, &[1]), (&tx3, &[0]), 2, Capacity::shannons(400));
    let tx6 = build_tx((&tx3, &[0]), (&tx3, &[]), 2, Capacity::shannons(10000));

    let txs = vec![tx1, tx2, tx3, tx4, tx5, tx6];

    let mut block_template = shared
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();

    // proposal txs
    {
        while Into::<u64>::into(block_template.number) != 1 {
            block_template = shared
                .get_block_template(None, None, None)
                .unwrap()
                .unwrap()
        }

        let block: Block = block_template.clone().into();
        let block = block
            .as_advanced_builder()
            .proposals(
                txs.iter()
                    .map(|tx| tx.proposal_short_id())
                    .collect::<Vec<_>>(),
            )
            .build();
        chain_controller
            .internal_process_block(Arc::new(block), Switch::DISABLE_ALL)
            .unwrap();
    }

    // skip gap
    {
        while Into::<u64>::into(block_template.number) != 2 {
            block_template = shared
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
    for tx in &txs {
        let ret = tx_pool.submit_local_tx(tx.clone()).unwrap();
        assert!(ret.is_ok(), "submit {} {:?}", tx.proposal_short_id(), ret);
    }

    let mut tx_pool_info = tx_pool.get_tx_pool_info().unwrap();
    while tx_pool_info.proposed_size != txs.len() {
        tx_pool_info = tx_pool.get_tx_pool_info().unwrap()
    }

    // get block template with txs
    while !(Into::<u64>::into(block_template.number) == 3 && block_template.transactions.len() == 6)
    {
        block_template = shared
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap()
    }

    let block: Block = block_template.into();
    let block = block.as_advanced_builder().build();

    for (index, tx) in block.transactions().iter().skip(1).enumerate() {
        assert_eq!(tx.proposal_short_id(), txs[index].proposal_short_id());
    }
}

#[test]
fn test_package_txs_with_deps_priority() {
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

    let tx1 = build_tx(
        (&issue_tx, &[0]),
        (&issue_tx, &[1]),
        2,
        Capacity::shannons(400),
    );
    let tx2 = build_tx(
        (&issue_tx, &[1]),
        (&issue_tx, &[]),
        2,
        Capacity::shannons(10000),
    );

    let txs = vec![tx2.clone(), tx1];

    for tx in &txs {
        let ret = tx_pool.submit_local_tx(tx.clone()).unwrap();
        assert!(ret.is_ok(), "submit {} {:?}", tx.proposal_short_id(), ret);
    }

    let mut block_template = shared
        .get_block_template(None, None, None)
        .unwrap()
        .unwrap();

    // proposal txs
    {
        while !(Into::<u64>::into(block_template.number) == 1
            && block_template.proposals.len() == 2)
        {
            block_template = shared
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

    // skip gap
    {
        while Into::<u64>::into(block_template.number) != 2 {
            block_template = shared
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

    let mut tx_pool_info = tx_pool.get_tx_pool_info().unwrap();
    while tx_pool_info.tip_number != 2 {
        tx_pool_info = tx_pool.get_tx_pool_info().unwrap()
    }

    // get block template with txs
    while !(Into::<u64>::into(block_template.number) == 3 && block_template.transactions.len() == 1)
    {
        block_template = shared
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap()
    }

    let block: Block = block_template.into();
    let block = block.as_advanced_builder().build();
    // tx1 will be discard
    assert_eq!(block.transactions()[1], tx2);
}
