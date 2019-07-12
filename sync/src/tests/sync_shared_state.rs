use crate::block_status::BlockStatus;
use crate::tests::util::{build_chain, inherit_block};
use crate::SyncSharedState;
use ckb_chain::chain::ChainService;
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::Capacity;
use ckb_db::MemoryKeyValueDB;
use ckb_network::PeerIndex;
use ckb_notify::NotifyService;
use ckb_shared::shared::SharedBuilder;
use ckb_store::ChainStore;
use ckb_test_chain_utils::always_success_cellbase;
use std::sync::Arc;

#[test]
fn test_insert_new_block() {
    let (shared, chain) = build_chain(2);
    let new_block = {
        let tip_number = shared.tip_header().number();
        let next_block = inherit_block(shared.shared(), tip_number).build();
        Arc::new(next_block)
    };

    assert_eq!(
        shared
            .insert_new_block(&chain, PeerIndex::new(1), Arc::clone(&new_block))
            .expect("insert valid block"),
        true,
    );
    assert_eq!(
        shared
            .insert_new_block(&chain, PeerIndex::new(1), Arc::clone(&new_block))
            .expect("insert duplicated valid block"),
        false,
    );
}

#[test]
fn test_insert_invalid_block() {
    let (shared, chain) = build_chain(2);
    let invalid_block = {
        let tip_number = shared.tip_header().number();
        let invalid_cellbase = always_success_cellbase(tip_number, Capacity::zero());
        let next_block = inherit_block(shared.shared(), tip_number)
            .transaction(invalid_cellbase)
            .build();
        Arc::new(next_block)
    };

    assert!(shared
        .insert_new_block(&chain, PeerIndex::new(1), Arc::clone(&invalid_block))
        .is_err(),);
}

#[test]
fn test_insert_parent_unknown_block() {
    let (shared1, _) = build_chain(2);
    let (shared, chain) = {
        let shared = SharedBuilder::<MemoryKeyValueDB>::new()
            .consensus(shared1.consensus().clone())
            .build()
            .unwrap();
        let chain_controller = {
            let notify_controller = NotifyService::default().start::<&str>(None);
            let chain_service = ChainService::new(shared.clone(), notify_controller);
            chain_service.start::<&str>(None)
        };
        (SyncSharedState::new(shared), chain_controller)
    };

    let block = shared1
        .store()
        .get_block(shared1.tip_header().hash())
        .unwrap();
    let parent = {
        let parent = shared1
            .store()
            .get_block(block.header().parent_hash())
            .unwrap();
        Arc::new(parent)
    };
    let invalid_orphan = {
        let invalid_orphan = BlockBuilder::from_block(block.clone())
            .header_builder(HeaderBuilder::from_header(block.header().clone()).number(1000))
            .build();
        Arc::new(invalid_orphan)
    };
    let valid_orphan = Arc::new(block);
    let valid_hash = valid_orphan.header().hash();
    let invalid_hash = invalid_orphan.header().hash();
    let parent_hash = parent.header().hash();

    assert_eq!(
        shared
            .insert_new_block(&chain, PeerIndex::new(1), Arc::clone(&valid_orphan))
            .expect("insert orphan block"),
        false,
    );
    assert_eq!(
        shared
            .insert_new_block(&chain, PeerIndex::new(1), Arc::clone(&invalid_orphan))
            .expect("insert orphan block"),
        false,
    );
    assert_eq!(
        shared.get_block_status(valid_hash),
        BlockStatus::BLOCK_RECEIVED
    );
    assert_eq!(
        shared.get_block_status(invalid_hash),
        BlockStatus::BLOCK_RECEIVED
    );

    // After inserting parent of an orphan block
    assert_eq!(
        shared
            .insert_new_block(&chain, PeerIndex::new(2), Arc::clone(&parent))
            .expect("insert parent of orphan block"),
        true,
    );
    assert_eq!(
        shared.get_block_status(valid_hash),
        BlockStatus::BLOCK_STORED
    );
    assert_eq!(
        shared.get_block_status(invalid_hash),
        BlockStatus::BLOCK_INVALID
    );
    assert_eq!(
        shared.get_block_status(parent_hash),
        BlockStatus::BLOCK_STORED
    );

    assert_eq!(
        shared.get_block_status(valid_hash),
        BlockStatus::BLOCK_STORED,
    );
    assert_eq!(
        shared.get_block_status(invalid_hash),
        BlockStatus::BLOCK_INVALID,
    );
}
