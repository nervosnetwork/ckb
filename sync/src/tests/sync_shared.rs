use crate::tests::util::{build_chain, inherit_block};
use crate::SyncShared;
use ckb_chain::chain::ChainService;
use ckb_launcher::SharedBuilder;
use ckb_shared::block_status::BlockStatus;
use ckb_store::{self, ChainStore};
use ckb_test_chain_utils::always_success_cellbase;
use ckb_types::core::{BlockBuilder, BlockView, Capacity};
use ckb_types::prelude::*;
use std::sync::Arc;

#[test]
fn test_insert_new_block() {
    let (shared, chain) = build_chain(2);
    let new_block = {
        let tip_hash = shared.active_chain().tip_header().hash();
        let next_block = inherit_block(shared.shared(), &tip_hash).build();
        Arc::new(next_block)
    };

    assert!(shared
        .insert_new_block(&chain, Arc::clone(&new_block))
        .expect("insert valid block"),);
    assert!(!shared
        .insert_new_block(&chain, Arc::clone(&new_block))
        .expect("insert duplicated valid block"),);
}

#[test]
fn test_insert_invalid_block() {
    let (shared, chain) = build_chain(2);
    let invalid_block = {
        let active_chain = shared.active_chain();
        let tip_number = active_chain.tip_number();
        let tip_hash = active_chain.tip_hash();
        let invalid_cellbase =
            always_success_cellbase(tip_number, Capacity::zero(), shared.consensus());
        let next_block = inherit_block(shared.shared(), &tip_hash)
            .transaction(invalid_cellbase)
            .build();
        Arc::new(next_block)
    };

    assert!(shared
        .insert_new_block(&chain, Arc::clone(&invalid_block))
        .is_err(),);
}

#[test]
fn test_insert_parent_unknown_block() {
    let (shared1, _) = build_chain(2);
    let (shared, chain) = {
        let (shared, mut pack) = SharedBuilder::with_temp_db()
            .consensus(shared1.consensus().clone())
            .build()
            .unwrap();
        let chain_controller = {
            let chain_service = ChainService::new(shared.clone(), pack.take_proposal_table());
            chain_service.start::<&str>(None)
        };
        (
            SyncShared::new(shared, Default::default(), pack.take_relay_tx_receiver()),
            chain_controller,
        )
    };

    let block = shared1
        .store()
        .get_block(&shared1.active_chain().tip_header().hash())
        .unwrap();
    let parent = {
        let parent = shared1
            .store()
            .get_block(&block.header().parent_hash())
            .unwrap();
        Arc::new(parent)
    };
    let invalid_orphan = {
        let invalid_orphan = block
            .as_advanced_builder()
            .header(block.header())
            .number(1000.pack())
            .build();

        Arc::new(invalid_orphan)
    };
    let valid_orphan = Arc::new(block);
    let valid_hash = valid_orphan.header().hash();
    let invalid_hash = invalid_orphan.header().hash();
    let parent_hash = parent.header().hash();

    assert!(!shared
        .insert_new_block(&chain, Arc::clone(&valid_orphan))
        .expect("insert orphan block"),);
    assert!(!shared
        .insert_new_block(&chain, Arc::clone(&invalid_orphan))
        .expect("insert orphan block"),);
    assert_eq!(
        shared.active_chain().get_block_status(&valid_hash),
        BlockStatus::BLOCK_RECEIVED
    );
    assert_eq!(
        shared.active_chain().get_block_status(&invalid_hash),
        BlockStatus::BLOCK_RECEIVED
    );

    // After inserting parent of an orphan block
    assert!(shared
        .insert_new_block(&chain, Arc::clone(&parent))
        .expect("insert parent of orphan block"),);
    assert_eq!(
        shared.active_chain().get_block_status(&valid_hash),
        BlockStatus::BLOCK_VALID
    );
    assert_eq!(
        shared.active_chain().get_block_status(&invalid_hash),
        BlockStatus::BLOCK_INVALID
    );
    assert_eq!(
        shared.active_chain().get_block_status(&parent_hash),
        BlockStatus::BLOCK_VALID
    );
}

#[test]
fn test_switch_valid_fork() {
    let (shared, chain) = build_chain(4);
    let make_valid_block = |shared, parent_hash| -> BlockView {
        let header = inherit_block(shared, &parent_hash).build().header();
        let timestamp = header.timestamp() + 3;
        let cellbase = inherit_block(shared, &parent_hash).build().transactions()[0].clone();
        BlockBuilder::default()
            .header(header)
            .timestamp(timestamp.pack())
            .transaction(cellbase)
            .build()
    };

    // Insert the valid fork. The fork blocks would not been verified until the fork switches as
    // the main chain. And `block_status_map` would mark the fork blocks as `BLOCK_STORED`
    let block_number = 1;
    let mut parent_hash = shared.store().get_block_hash(block_number).unwrap();
    for number in 0..=block_number {
        let block_hash = shared.store().get_block_hash(number).unwrap();
        shared.store().get_block(&block_hash).unwrap();
    }
    let mut valid_fork = Vec::new();
    for _ in 2..shared.active_chain().tip_number() {
        let block = make_valid_block(shared.shared(), parent_hash.clone());
        assert!(shared
            .insert_new_block(&chain, Arc::new(block.clone()))
            .expect("insert fork"),);

        parent_hash = block.header().hash();
        valid_fork.push(block);
    }
    for block in valid_fork.iter() {
        assert_eq!(
            shared
                .active_chain()
                .get_block_status(&block.header().hash()),
            BlockStatus::BLOCK_STORED,
        );
    }

    let tip_number = shared.active_chain().tip_number();
    // Make the fork switch as the main chain.
    for _ in tip_number..tip_number + 2 {
        let block = inherit_block(shared.shared(), &parent_hash.clone()).build();
        assert!(shared
            .insert_new_block(&chain, Arc::new(block.clone()))
            .expect("insert fork"),);

        parent_hash = block.header().hash();
        valid_fork.push(block);
    }
    for block in valid_fork.iter() {
        assert_eq!(
            shared
                .active_chain()
                .get_block_status(&block.header().hash()),
            BlockStatus::BLOCK_VALID,
        );
    }
}
