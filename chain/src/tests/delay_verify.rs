use crate::tests::util::{
    create_cellbase, create_multi_outputs_transaction, create_transaction,
    create_transaction_with_out_point, start_chain, MockChain,
};
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::cell::UnresolvableError;
use ckb_core::header::HeaderBuilder;
use ckb_core::transaction::OutPoint;
use ckb_shared::error::SharedError;
use numext_fixed_uint::U256;
use std::sync::Arc;
use test_chain_utils::build_block;

#[test]
fn test_dead_cell_in_same_block() {
    let (chain_controller, _shared, parent) = start_chain(None);
    let final_number = 20;
    let switch_fork_number = 10;

    let mut chain1 = MockChain::new(parent.clone());
    let mut chain2 = MockChain::new(parent.clone());

    for _ in 1..final_number {
        chain1.gen_empty_block(100u64);
    }

    for _ in 1..switch_fork_number {
        chain2.gen_empty_block(99u64);
    }

    let last_cell_base = &chain2.blocks().last().unwrap().transactions()[0];
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_transaction(&tx1_hash, 2);
    let tx3 = create_transaction(&tx1_hash, 3);
    let txs = vec![tx1, tx2, tx3];

    chain2.gen_block_with_proposal_txs(txs.clone());
    chain2.gen_empty_block(20000u64);
    chain2.gen_block_with_commit_txs(txs.clone());

    for _ in (switch_fork_number + 3)..final_number {
        chain2.gen_empty_block(20000u64);
    }

    for block in chain1.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }

    for block in chain2.blocks().iter().take(switch_fork_number + 1) {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }

    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Dead(OutPoint::new_cell(
            tx1_hash, 0
        ))),
        chain_controller
            .process_block(
                Arc::new(chain2.blocks()[switch_fork_number + 1].clone()),
                true
            )
            .unwrap_err()
            .downcast()
            .unwrap()
    )
}

#[test]
fn test_dead_cell_in_different_block() {
    let (chain_controller, _shared, parent) = start_chain(None);
    let final_number = 20;
    let switch_fork_number = 10;

    let mut chain1 = MockChain::new(parent.clone());
    let mut chain2 = MockChain::new(parent.clone());

    for _ in 1..final_number {
        chain1.gen_empty_block(100u64);
    }

    for _ in 1..switch_fork_number {
        chain2.gen_empty_block(100u64);
    }

    let last_cell_base = &chain2.tip().transactions()[0];
    let tx1 = create_multi_outputs_transaction(&last_cell_base, vec![0], 2, vec![1]);
    let tx1_hash = tx1.hash();
    let tx2 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![2]);
    let tx3 = create_multi_outputs_transaction(&tx1, vec![0], 2, vec![3]);

    chain2.gen_block_with_proposal_txs(vec![tx1.clone(), tx2.clone(), tx3.clone()]);
    chain2.gen_empty_block(20000u64);
    chain2.gen_block_with_commit_txs(vec![tx1.clone(), tx2.clone()]);
    chain2.gen_block_with_commit_txs(vec![tx3.clone()]);
    for _ in (switch_fork_number + 4)..final_number {
        chain2.gen_empty_block(20000u64);
    }

    for block in chain1.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }

    for block in chain2.blocks().iter().take(switch_fork_number + 2) {
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
            .process_block(
                Arc::new(chain2.blocks()[switch_fork_number + 2].clone()),
                true
            )
            .unwrap_err()
            .downcast()
            .unwrap()
    );
}

#[test]
fn test_invalid_out_point_index_in_same_block() {
    let (chain_controller, _shared, parent) = start_chain(None);
    let final_number = 20;
    let switch_fork_number = 10;

    let mut chain1 = MockChain::new(parent.clone());
    let mut chain2 = MockChain::new(parent.clone());

    for _ in 1..final_number {
        chain1.gen_empty_block(100u64);
    }

    for _ in 1..switch_fork_number {
        chain2.gen_empty_block(99u64);
    }

    let last_cell_base = &chain2.blocks().last().unwrap().transactions()[0];
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_transaction(&tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new_cell(tx1_hash.clone(), 1), 3);
    let txs = vec![tx1, tx2, tx3];

    chain2.gen_block_with_proposal_txs(txs.clone());
    chain2.gen_empty_block(20000u64);
    chain2.gen_block_with_commit_txs(txs.clone());
    for _ in (switch_fork_number + 3)..final_number {
        chain2.gen_empty_block(20000u64);
    }

    for block in chain1.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }

    for block in chain2.blocks().iter().take(switch_fork_number + 1) {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }

    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Unknown(vec![OutPoint::new_cell(
            tx1_hash, 1,
        )])),
        chain_controller
            .process_block(
                Arc::new(chain2.blocks()[switch_fork_number + 1].clone()),
                true
            )
            .unwrap_err()
            .downcast()
            .unwrap()
    )
}

#[test]
fn test_invalid_out_point_index_in_different_blocks() {
    let (chain_controller, _shared, parent) = start_chain(None);
    let final_number = 20;
    let switch_fork_number = 10;

    let mut chain1 = MockChain::new(parent.clone());
    let mut chain2 = MockChain::new(parent.clone());

    for _ in 1..final_number {
        chain1.gen_empty_block(100u64);
    }

    for _ in 1..switch_fork_number {
        chain2.gen_empty_block(99u64);
    }

    let last_cell_base = &chain2.blocks().last().unwrap().transactions()[0];
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(tx1_hash, 2);
    // create an invalid OutPoint index
    let tx3 = create_transaction_with_out_point(OutPoint::new_cell(tx1_hash.clone(), 1), 3);

    chain2.gen_block_with_proposal_txs(vec![tx1.clone(), tx2.clone(), tx3.clone()]);
    chain2.gen_empty_block(20000u64);
    chain2.gen_block_with_commit_txs(vec![tx1.clone(), tx2.clone()]);
    chain2.gen_block_with_commit_txs(vec![tx3.clone()]);

    for _ in (switch_fork_number + 4)..final_number {
        chain2.gen_empty_block(20000u64);
    }

    for block in chain1.blocks() {
        chain_controller
            .process_block(Arc::new(block.clone()), true)
            .expect("process block ok");
    }

    for block in chain2.blocks().iter().take(switch_fork_number + 2) {
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
            .process_block(
                Arc::new(chain2.blocks()[switch_fork_number + 2].clone()),
                true
            )
            .unwrap_err()
            .downcast()
            .unwrap()
    );
}

#[test]
fn test_full_dead_transaction() {
    let (chain_controller, _shared, mut parent) = start_chain(None);
    let final_number = 20;
    let switch_fork_number = 10;
    let proposal_number = 3;

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let difficulty = parent.difficulty().to_owned();
    let block = build_block!(
        from_header_builder: {
            parent_hash: parent.hash().to_owned(),
            number: parent.number() + 1,
            difficulty: difficulty + U256::from(100u64),
        },
        transaction: create_cellbase(parent.number() + 1),
    );
    chain1.push(block.clone());
    chain2.push(block.clone());
    let root_tx = &block.transactions()[0];
    let tx1 = create_multi_outputs_transaction(&root_tx, vec![0], 1, vec![1]);

    parent = block.header().to_owned();
    for i in 2..switch_fork_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = if i == proposal_number {
            build_block!(
                from_header_builder: {
                    parent_hash: parent.hash().to_owned(),
                    number: parent.number() + 1,
                    difficulty: difficulty + U256::from(100u64),
                },
                transaction: create_cellbase(parent.number() + 1),
                proposals: vec![tx1.proposal_short_id()],
            )
        } else if i == proposal_number + 2 {
            build_block!(
                from_header_builder: {
                    parent_hash: parent.hash().to_owned(),
                    number: parent.number() + 1,
                    difficulty: difficulty + U256::from(100u64),
                },
                transaction: create_cellbase(parent.number() + 1),
                transactions: vec![tx1.clone()],
            )
        } else {
            build_block!(
                from_header_builder: {
                    parent_hash: parent.hash().to_owned(),
                    number: parent.number() + 1,
                    difficulty: difficulty + U256::from(100u64),
                },
                transaction: create_cellbase(parent.number() + 1),
            )
        };
        chain1.push(new_block.clone());
        chain2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    let tx2 = create_multi_outputs_transaction(&tx1, vec![0], 1, vec![2]);
    let tx3 = create_multi_outputs_transaction(&tx2, vec![0], 1, vec![3]);

    for i in switch_fork_number..final_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = if i == final_number - 3 {
            build_block!(
                from_header_builder: {
                    parent_hash: parent.hash().to_owned(),
                    number: parent.number() + 1,
                    difficulty: difficulty + U256::from(100u64),
                },
                transaction: create_cellbase(parent.number() + 1),
                proposals: vec![tx2.proposal_short_id(), tx3.proposal_short_id()],
            )
        } else if i == final_number - 1 {
            build_block!(
                from_header_builder: {
                    parent_hash: parent.hash().to_owned(),
                    number: parent.number() + 1,
                    difficulty: difficulty + U256::from(100u64),
                },
                transaction: create_cellbase(parent.number() + 1),
                transactions: vec![tx2.clone(), tx3.clone()],
            )
        } else {
            build_block!(
                from_header_builder: {
                    parent_hash: parent.hash().to_owned(),
                    number: parent.number() + 1,
                    difficulty: difficulty + U256::from(100u64),
                },
                transaction: create_cellbase(parent.number() + 1),
            )
        };
        chain1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    parent = chain2.last().unwrap().header().clone();
    for i in switch_fork_number..final_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = if i == final_number - 3 {
            build_block!(
                from_header_builder: {
                    parent_hash: parent.hash().to_owned(),
                    number: parent.number() + 1,
                    difficulty: difficulty + U256::from(101u64),
                },
                transaction: create_cellbase(parent.number() + 1),
                proposals: vec![tx2.proposal_short_id(), tx3.proposal_short_id()],
            )
        } else if i == final_number - 1 {
            build_block!(
                from_header_builder: {
                    parent_hash: parent.hash().to_owned(),
                    number: parent.number() + 1,
                    difficulty: difficulty + U256::from(101u64),
                },
                transaction: create_cellbase(parent.number() + 1),
                transactions: vec![tx2.clone(), tx3.clone()],
            )
        } else {
            build_block!(
                from_header_builder: {
                    parent_hash: parent.hash().to_owned(),
                    number: parent.number() + 1,
                    difficulty: difficulty + U256::from(101u64),
                },
                transaction: create_cellbase(parent.number() + 1),
            )
        };
        chain2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    for block in chain1 {
        chain_controller
            .process_block(Arc::new(block), true)
            .expect("process block ok");
    }

    for block in chain2 {
        chain_controller
            .process_block(Arc::new(block), true)
            .expect("process block ok");
    }
}
