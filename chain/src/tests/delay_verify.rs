use crate::tests::util::{
    create_transaction, create_transaction_with_out_point, start_chain, MockChain,
};
use ckb_core::cell::UnresolvableError;
use ckb_core::transaction::OutPoint;
use ckb_shared::error::SharedError;
use std::sync::Arc;

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

    let last_cell_base = &chain2.blocks().last().unwrap().transactions()[0];
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(tx1_hash, 2);
    let tx3 = create_transaction(tx1_hash, 3);

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
