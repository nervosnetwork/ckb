use crate::tests::util::start_chain;
use ckb_error::assert_error_eq;
use ckb_store::ChainStore;
use ckb_test_chain_utils::{
    MockChain, MockStore, create_cellbase, create_multi_outputs_transaction, create_transaction,
    create_transaction_with_out_point, dao_data,
};
use ckb_types::core::error::OutPointError;
use ckb_types::prelude::*;
use ckb_types::{core::BlockBuilder, packed::OutPoint};
use ckb_verification_traits::Switch;
use std::sync::Arc;

#[test]
fn test_dead_cell_in_same_block() {
    let (chain_controller, shared, parent) = start_chain(None);
    let final_number = 20;
    let switch_fork_number = 10;

    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain1 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain2 = MockChain::new(parent, shared.consensus());

    for _ in 1..final_number {
        chain1.gen_empty_block_with_inc_diff(100u64, &mock_store);
    }

    for _ in 1..switch_fork_number {
        chain2.gen_empty_block_with_inc_diff(99u64, &mock_store);
    }

    let last_cellbase = &shared.consensus().genesis_block().transactions()[1];
    let tx1 = create_transaction(&last_cellbase.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(&tx1_hash, 2);
    let tx3 = create_transaction(&tx1_hash, 3);
    let txs = vec![tx1, tx2, tx3];

    chain2.gen_block_with_proposal_txs(txs.clone(), &mock_store);
    chain2.gen_empty_block_with_inc_diff(20000u64, &mock_store);
    chain2.gen_block_with_commit_txs(txs, &mock_store, true);

    for _ in (switch_fork_number + 3)..final_number {
        chain2.gen_empty_block_with_inc_diff(20000u64, &mock_store);
    }

    for block in chain1.blocks() {
        chain_controller
            .blocking_process_block_with_switch(
                Arc::new(block.clone()),
                Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
            )
            .expect("process block ok");
    }

    for block in chain2.blocks().iter().take(switch_fork_number + 1) {
        chain_controller
            .blocking_process_block_with_switch(
                Arc::new(block.clone()),
                Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
            )
            .expect("process block ok");
    }

    assert_error_eq!(
        OutPointError::Dead(OutPoint::new(tx1_hash, 0)),
        chain_controller
            .blocking_process_block_with_switch(
                Arc::new(chain2.blocks()[switch_fork_number + 1].clone()),
                Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
            )
            .unwrap_err(),
    )
}

#[test]
fn test_dead_cell_in_different_block() {
    let (chain_controller, shared, parent) = start_chain(None);
    let final_number = 20;
    let switch_fork_number = 10;

    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain1 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain2 = MockChain::new(parent, shared.consensus());

    for _ in 1..final_number {
        chain1.gen_empty_block_with_inc_diff(100u64, &mock_store);
    }

    for _ in 1..switch_fork_number {
        chain2.gen_empty_block_with_inc_diff(100u64, &mock_store);
    }

    let last_cellbase = &shared.consensus().genesis_block().transactions()[1];
    let tx1 = create_multi_outputs_transaction(last_cellbase, vec![0], 2, vec![1]);
    let tx1_hash = tx1.hash();
    let tx2 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![2]);
    let tx3 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![3]);

    chain2.gen_block_with_proposal_txs(vec![tx1.clone(), tx2.clone(), tx3.clone()], &mock_store);
    chain2.gen_empty_block_with_inc_diff(20000u64, &mock_store);
    chain2.gen_block_with_commit_txs(vec![tx1, tx2], &mock_store, false);
    chain2.gen_block_with_commit_txs(vec![tx3], &mock_store, false);
    for _ in (switch_fork_number + 4)..final_number {
        chain2.gen_empty_block_with_inc_diff(20000u64, &mock_store);
    }

    for block in chain1.blocks() {
        chain_controller
            .blocking_process_block_with_switch(
                Arc::new(block.clone()),
                Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
            )
            .expect("process block ok");
    }

    for block in chain2.blocks().iter().take(switch_fork_number + 2) {
        chain_controller
            .blocking_process_block_with_switch(
                Arc::new(block.clone()),
                Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
            )
            .expect("process block ok");
    }

    assert_error_eq!(
        OutPointError::Unknown(OutPoint::new(tx1_hash, 0)),
        chain_controller
            .blocking_process_block_with_switch(
                Arc::new(chain2.blocks()[switch_fork_number + 2].clone()),
                Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
            )
            .unwrap_err(),
    );
}

#[test]
fn test_invalid_out_point_index_in_same_block() {
    let (chain_controller, shared, parent) = start_chain(None);
    let final_number = 20;
    let switch_fork_number = 10;

    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain1 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain2 = MockChain::new(parent, shared.consensus());

    for _ in 1..final_number {
        chain1.gen_empty_block_with_inc_diff(100u64, &mock_store);
    }

    for _ in 1..switch_fork_number {
        chain2.gen_empty_block_with_inc_diff(99u64, &mock_store);
    }

    let last_cellbase = &shared.consensus().genesis_block().transactions()[1];
    let tx1 = create_transaction(&last_cellbase.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(&tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new(tx1_hash.clone(), 1), 3);
    let txs = vec![tx1, tx2, tx3];

    chain2.gen_block_with_proposal_txs(txs.clone(), &mock_store);
    chain2.gen_empty_block_with_inc_diff(20000u64, &mock_store);
    chain2.gen_block_with_commit_txs(txs, &mock_store, true);
    for _ in (switch_fork_number + 3)..final_number {
        chain2.gen_empty_block_with_inc_diff(20000u64, &mock_store);
    }

    for block in chain1.blocks() {
        chain_controller
            .blocking_process_block_with_switch(
                Arc::new(block.clone()),
                Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
            )
            .expect("process block ok");
    }

    for block in chain2.blocks().iter().take(switch_fork_number + 1) {
        chain_controller
            .blocking_process_block_with_switch(
                Arc::new(block.clone()),
                Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
            )
            .expect("process block ok");
    }

    assert_error_eq!(
        OutPointError::Unknown(OutPoint::new(tx1_hash, 1)),
        chain_controller
            .blocking_process_block_with_switch(
                Arc::new(chain2.blocks()[switch_fork_number + 1].clone()),
                Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
            )
            .unwrap_err(),
    )
}

#[test]
fn test_invalid_out_point_index_in_different_blocks() {
    let (chain_controller, shared, parent) = start_chain(None);
    let final_number = 20;
    let switch_fork_number = 10;

    let mock_store = MockStore::new(&parent, shared.store());
    let mut chain1 = MockChain::new(parent.clone(), shared.consensus());
    let mut chain2 = MockChain::new(parent, shared.consensus());

    for _ in 1..final_number {
        chain1.gen_empty_block_with_inc_diff(100u64, &mock_store);
    }

    for _ in 1..switch_fork_number {
        chain2.gen_empty_block_with_inc_diff(99u64, &mock_store);
    }

    let last_cellbase = &shared.consensus().genesis_block().transactions()[1];
    let tx1 = create_transaction(&last_cellbase.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(&tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new(tx1_hash.clone(), 1), 3);

    chain2.gen_block_with_proposal_txs(vec![tx1.clone(), tx2.clone(), tx3.clone()], &mock_store);
    chain2.gen_empty_block_with_inc_diff(20000u64, &mock_store);
    chain2.gen_block_with_commit_txs(vec![tx1, tx2], &mock_store, false);
    chain2.gen_block_with_commit_txs(vec![tx3], &mock_store, true);

    for _ in (switch_fork_number + 4)..final_number {
        chain2.gen_empty_block_with_inc_diff(20000u64, &mock_store);
    }

    for block in chain1.blocks() {
        chain_controller
            .blocking_process_block_with_switch(
                Arc::new(block.clone()),
                Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
            )
            .expect("process block ok");
    }

    for block in chain2.blocks().iter().take(switch_fork_number + 2) {
        chain_controller
            .blocking_process_block_with_switch(
                Arc::new(block.clone()),
                Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
            )
            .expect("process block ok");
    }

    assert_error_eq!(
        OutPointError::Unknown(OutPoint::new(tx1_hash, 1)),
        chain_controller
            .blocking_process_block_with_switch(
                Arc::new(chain2.blocks()[switch_fork_number + 2].clone()),
                Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
            )
            .unwrap_err(),
    );
}

#[test]
fn test_full_dead_transaction() {
    let (chain_controller, shared, mut parent) = start_chain(None);
    let final_number = 20;
    let switch_fork_number = 10;
    let proposal_number = 3;

    let mock_store = MockStore::new(&parent, shared.store());

    let cellbase_tx = create_cellbase(&mock_store, shared.consensus(), &parent);
    let dao = dao_data(
        shared.consensus(),
        &parent,
        &[cellbase_tx.clone()],
        &mock_store,
        false,
    );

    let compact_target = parent.compact_target();

    let epoch = shared
        .consensus()
        .next_epoch_ext(&parent, &shared.store().borrow_as_data_loader())
        .unwrap()
        .epoch();

    let block = BlockBuilder::default()
        .parent_hash(parent.hash())
        .number((parent.number() + 1).pack())
        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
        .compact_target((compact_target - 1).pack())
        .dao(dao)
        .transaction(cellbase_tx)
        .build();

    chain_controller
        .blocking_process_block_with_switch(
            Arc::new(block.clone()),
            Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
        )
        .expect("process block ok");

    mock_store.insert_block(&block, &epoch);
    let root_tx = &shared.consensus().genesis_block().transactions()[1];
    let tx1 = create_multi_outputs_transaction(root_tx, vec![0], 1, vec![1]);

    for is_new_chain in [false, true] {
        parent = block.header();
        for i in 2..switch_fork_number {
            let compact_target = parent.compact_target();

            let epoch = shared
                .consensus()
                .next_epoch_ext(&parent, &shared.store().borrow_as_data_loader())
                .unwrap()
                .epoch();

            let new_block = if i == proposal_number {
                let transactions = vec![create_cellbase(&mock_store, shared.consensus(), &parent)];
                let dao = dao_data(
                    shared.consensus(),
                    &parent,
                    &transactions,
                    &mock_store,
                    false,
                );
                BlockBuilder::default()
                    .parent_hash(parent.hash())
                    .number((parent.number() + 1).pack())
                    .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
                    .compact_target((compact_target - 1).pack())
                    .dao(dao)
                    .transactions(transactions)
                    .proposals(vec![tx1.proposal_short_id()])
                    .build()
            } else if i == proposal_number + 2 {
                let transactions = vec![
                    create_cellbase(&mock_store, shared.consensus(), &parent),
                    tx1.clone(),
                ];
                let dao = dao_data(
                    shared.consensus(),
                    &parent,
                    &transactions,
                    &mock_store,
                    false,
                );
                BlockBuilder::default()
                    .parent_hash(parent.hash())
                    .number((parent.number() + 1).pack())
                    .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
                    .compact_target((compact_target - 1).pack())
                    .dao(dao)
                    .transactions(transactions)
                    .build()
            } else {
                let transactions = vec![create_cellbase(&mock_store, shared.consensus(), &parent)];
                let dao = dao_data(
                    shared.consensus(),
                    &parent,
                    &transactions,
                    &mock_store,
                    false,
                );
                BlockBuilder::default()
                    .parent_hash(parent.hash())
                    .number((parent.number() + 1).pack())
                    .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
                    .compact_target((compact_target - 1).pack())
                    .dao(dao)
                    .transactions(transactions)
                    .build()
            };
            chain_controller
                .blocking_process_block_with_switch(
                    Arc::new(new_block.clone()),
                    Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
                )
                .expect("process block ok");
            mock_store.insert_block(&new_block, &epoch);
            parent = new_block.header().to_owned();
        }

        let tx2 = create_multi_outputs_transaction(&tx1, vec![0], 1, vec![2]);
        let tx3 = create_multi_outputs_transaction(&tx2, vec![0], 1, vec![3]);

        if !is_new_chain {
            for i in switch_fork_number..final_number {
                let compact_target = parent.compact_target();

                let epoch = shared
                    .consensus()
                    .next_epoch_ext(&parent, &shared.store().borrow_as_data_loader())
                    .unwrap()
                    .epoch();

                let new_block = if i == final_number - 3 {
                    let transactions =
                        vec![create_cellbase(&mock_store, shared.consensus(), &parent)];
                    let dao = dao_data(
                        shared.consensus(),
                        &parent,
                        &transactions,
                        &mock_store,
                        false,
                    );
                    BlockBuilder::default()
                        .parent_hash(parent.hash())
                        .number((parent.number() + 1).pack())
                        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
                        .compact_target((compact_target - 1).pack())
                        .dao(dao)
                        .transactions(transactions)
                        .proposals(vec![tx2.proposal_short_id(), tx3.proposal_short_id()])
                        .build()
                } else if i == final_number - 1 {
                    let transactions = vec![
                        create_cellbase(&mock_store, shared.consensus(), &parent),
                        tx2.clone(),
                        tx3.clone(),
                    ];
                    let dao = dao_data(
                        shared.consensus(),
                        &parent,
                        &transactions,
                        &mock_store,
                        false,
                    );
                    BlockBuilder::default()
                        .parent_hash(parent.hash())
                        .number((parent.number() + 1).pack())
                        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
                        .compact_target((compact_target - 1).pack())
                        .dao(dao)
                        .transactions(transactions)
                        .build()
                } else {
                    let transactions =
                        vec![create_cellbase(&mock_store, shared.consensus(), &parent)];
                    let dao = dao_data(
                        shared.consensus(),
                        &parent,
                        &transactions,
                        &mock_store,
                        false,
                    );

                    BlockBuilder::default()
                        .parent_hash(parent.hash())
                        .number((parent.number() + 1).pack())
                        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
                        .compact_target((compact_target - 1).pack())
                        .dao(dao)
                        .transactions(transactions)
                        .build()
                };
                chain_controller
                    .blocking_process_block_with_switch(
                        Arc::new(new_block.clone()),
                        Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
                    )
                    .expect("process block ok");
                mock_store.insert_block(&new_block, &epoch);
                parent = new_block.header().to_owned();
            }
        } else {
            for i in switch_fork_number..final_number {
                let compact_target = parent.compact_target();
                let new_block = if i == final_number - 3 {
                    let transactions =
                        vec![create_cellbase(&mock_store, shared.consensus(), &parent)];
                    let dao = dao_data(
                        shared.consensus(),
                        &parent,
                        &transactions,
                        &mock_store,
                        false,
                    );
                    BlockBuilder::default()
                        .parent_hash(parent.hash())
                        .number((parent.number() + 1).pack())
                        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
                        .compact_target((compact_target - 1).pack())
                        .dao(dao)
                        .proposals(vec![tx2.proposal_short_id(), tx3.proposal_short_id()])
                        .transactions(transactions)
                        .build()
                } else if i == final_number - 1 {
                    let transactions = vec![
                        create_cellbase(&mock_store, shared.consensus(), &parent),
                        tx2.clone(),
                        tx3.clone(),
                    ];
                    let dao = dao_data(
                        shared.consensus(),
                        &parent,
                        &transactions,
                        &mock_store,
                        false,
                    );
                    BlockBuilder::default()
                        .parent_hash(parent.hash())
                        .number((parent.number() + 1).pack())
                        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
                        .compact_target((compact_target - 1).pack())
                        .dao(dao)
                        .transactions(transactions)
                        .build()
                } else {
                    let transactions =
                        vec![create_cellbase(&mock_store, shared.consensus(), &parent)];
                    let dao = dao_data(
                        shared.consensus(),
                        &parent,
                        &transactions,
                        &mock_store,
                        false,
                    );

                    BlockBuilder::default()
                        .parent_hash(parent.hash())
                        .number((parent.number() + 1).pack())
                        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
                        .compact_target((compact_target - 1).pack())
                        .dao(dao)
                        .transactions(transactions)
                        .build()
                };
                chain_controller
                    .blocking_process_block_with_switch(
                        Arc::new(new_block.clone()),
                        Switch::DISABLE_EPOCH | Switch::DISABLE_EXTENSION,
                    )
                    .expect("process block ok");
                mock_store.insert_block(&new_block, &epoch);
                parent = new_block.header().to_owned();
            }
        }
    }
}
