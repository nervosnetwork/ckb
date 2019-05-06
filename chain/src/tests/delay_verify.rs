use crate::tests::util::{create_transaction, gen_block, start_chain};
use ckb_core::block::Block;
use ckb_core::cell::UnresolvableError;
use ckb_core::transaction::OutPoint;
use ckb_shared::error::SharedError;
use ckb_traits::ChainProvider;
use numext_fixed_uint::U256;
use std::sync::Arc;

#[test]
fn test_dead_cell_in_same_block() {
    let (chain_controller, shared) = start_chain(None, true);
    let final_number = 20;
    let switch_fork_number = 10;

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        chain1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..switch_fork_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(99u64),
            vec![],
            vec![],
            vec![],
        );
        chain2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    let last_cell_base = &chain2.last().unwrap().transactions()[0];
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash().to_owned();
    let tx2 = create_transaction(&tx1_hash, 2);
    let tx3 = create_transaction(&tx1_hash, 3);
    let txs = vec![tx1, tx2, tx3];
    for i in switch_fork_number..final_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = if i == switch_fork_number {
            gen_block(
                &parent,
                difficulty + U256::from(20000u64),
                vec![],
                txs.clone(),
                vec![],
            )
        } else if i == switch_fork_number + 2 {
            gen_block(
                &parent,
                difficulty + U256::from(20000u64),
                txs.clone(),
                vec![],
                vec![],
            )
        } else {
            gen_block(
                &parent,
                difficulty + U256::from(20000u64),
                vec![],
                vec![],
                vec![],
            )
        };
        chain2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    for block in &chain1 {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }

    for block in chain2.iter().take(switch_fork_number + 1) {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }

    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Dead(OutPoint {
            tx_hash: tx1_hash,
            index: 0,
        })),
        chain_controller
            .process_block(Arc::new(chain2[switch_fork_number + 1].clone()))
            .unwrap_err()
            .downcast()
            .unwrap()
    )
}

#[test]
fn test_dead_cell_in_different_block() {
    let (chain_controller, shared) = start_chain(None, true);
    let final_number = 20;
    let switch_fork_number = 10;

    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..final_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(100u64),
            vec![],
            vec![],
            vec![],
        );
        chain1.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for _ in 1..switch_fork_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = gen_block(
            &parent,
            difficulty + U256::from(99u64),
            vec![],
            vec![],
            vec![],
        );
        chain2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    let last_cell_base = &chain2.last().unwrap().transactions()[0];
    let tx1 = create_transaction(last_cell_base.hash(), 1);
    let tx1_hash = tx1.hash();
    let tx2 = create_transaction(tx1_hash, 2);
    let tx3 = create_transaction(tx1_hash, 3);
    for i in switch_fork_number..final_number {
        let difficulty = parent.difficulty().to_owned();
        let new_block = if i == switch_fork_number {
            gen_block(
                &parent,
                difficulty + U256::from(20000u64),
                vec![],
                vec![tx1.clone(), tx2.clone(), tx3.clone()],
                vec![],
            )
        } else if i == switch_fork_number + 2 {
            gen_block(
                &parent,
                difficulty + U256::from(20000u64),
                vec![tx1.clone(), tx2.clone()],
                vec![],
                vec![],
            )
        } else if i == switch_fork_number + 3 {
            gen_block(
                &parent,
                difficulty + U256::from(20000u64),
                vec![tx3.clone()],
                vec![],
                vec![],
            )
        } else {
            gen_block(
                &parent,
                difficulty + U256::from(20000u64),
                vec![],
                vec![],
                vec![],
            )
        };
        chain2.push(new_block.clone());
        parent = new_block.header().to_owned();
    }

    for block in &chain1 {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }

    for block in chain2.iter().take(switch_fork_number + 2) {
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block ok");
    }

    assert_eq!(
        SharedError::UnresolvableTransaction(UnresolvableError::Dead(OutPoint {
            tx_hash: tx1_hash.to_owned(),
            index: 0,
        })),
        chain_controller
            .process_block(Arc::new(chain2[switch_fork_number + 2].clone()))
            .unwrap_err()
            .downcast()
            .unwrap()
    );
}
