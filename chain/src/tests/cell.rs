use crate::tests::util::{calculate_reward, create_always_success_tx, start_chain, MockStore};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_dao_utils::genesis_dao_data;
use ckb_shared::shared::Shared;
use ckb_store::{attach_block_cell, detach_block_cell, ChainStore};
use ckb_test_chain_utils::always_success_cell;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes,
        cell::{CellProvider, CellStatus},
        BlockBuilder, BlockView, Capacity, EpochNumberWithFraction, HeaderView, TransactionBuilder,
        TransactionView,
    },
    packed::{CellInput, CellOutputBuilder, OutPoint},
    utilities::DIFF_TWO,
};

const TX_FEE: Capacity = capacity_bytes!(10);

#[allow(clippy::int_plus_one)]
pub(crate) fn create_cellbase(
    parent: &HeaderView,
    store: &MockStore,
    consensus: &Consensus,
) -> TransactionView {
    let number = parent.number() + 1;
    let capacity = calculate_reward(store, consensus, parent);
    let builder = TransactionBuilder::default().input(CellInput::new_cellbase_input(number));

    if (parent.number() + 1) <= consensus.finalization_delay_length() {
        builder.build()
    } else {
        builder
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity.pack())
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
    shared: &Shared,
    store: &MockStore,
) -> BlockView {
    let number = parent_header.number() + 1;
    let consensus = shared.consensus();
    let cellbase = create_cellbase(parent_header, store, consensus);
    let mut txs = vec![cellbase];
    txs.extend_from_slice(&transactions);

    let last_epoch = store
        .0
        .get_block_epoch_index(&parent_header.hash())
        .and_then(|index| store.0.get_epoch_ext(&index))
        .unwrap();
    let epoch = store
        .0
        .next_epoch_ext(shared.consensus(), &last_epoch, &parent_header)
        .unwrap_or(last_epoch);

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

pub(crate) fn create_transaction(parent: &TransactionView, index: u32) -> TransactionView {
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
                .build(),
        )
        .input(CellInput::new(OutPoint::new(parent.hash(), index), 0))
        .output_data(Bytes::new().pack())
        .build()
}

#[test]
fn test_block_cells_update() {
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

    let (_chain_controller, shared, parent) = start_chain(Some(consensus));
    let mock_store = MockStore::new(&parent, shared.store());

    let tx0 = create_transaction(&issue_tx, 0);
    let tx1 = create_transaction(&tx0, 0);
    let tx2 = create_transaction(&tx1, 0);
    let tx3 = create_transaction(&tx2, 0);

    let block = gen_block(&parent, vec![tx0, tx1, tx2, tx3], &shared, &mock_store);

    let db_txn = shared.store().begin_transaction();
    db_txn.insert_block(&block).unwrap();
    db_txn.attach_block(&block).unwrap();

    attach_block_cell(&db_txn, &block).unwrap();
    let txn_cell_provider = db_txn.cell_provider();

    // ensure tx0-2 outputs is spent after attach_block_cell
    for tx in block.transactions()[1..4].iter() {
        for pt in tx.output_pts() {
            // full spent
            assert_eq!(txn_cell_provider.cell(&pt, false), CellStatus::Unknown);
        }
    }

    // ensure tx3 outputs is unspent after attach_block_cell
    for pt in block.transactions()[4].output_pts() {
        assert!(txn_cell_provider.cell(&pt, false).is_live());
    }

    // ensure issue_tx outputs is spent after attach_block_cell
    assert_eq!(
        txn_cell_provider.cell(&issue_tx.output_pts()[0], false),
        CellStatus::Unknown
    );

    detach_block_cell(&db_txn, &block).unwrap();

    // ensure tx0-3 outputs is unknown after detach_block_cell
    for tx in block.transactions()[1..=4].iter() {
        for pt in tx.output_pts() {
            assert_eq!(txn_cell_provider.cell(&pt, false), CellStatus::Unknown);
        }
    }

    // ensure issue_tx outputs is back to live after detach_block_cell
    assert!(txn_cell_provider
        .cell(&issue_tx.output_pts()[0], false)
        .is_live());
}
