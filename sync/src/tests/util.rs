use crate::SyncShared;
use ckb_chain::{
    chain::{ChainController, ChainService},
    switch::Switch,
};

use ckb_dao::DaoCalculator;
use ckb_shared::{
    shared::{Shared, SharedBuilder},
    Snapshot,
};
use ckb_store::ChainStore;
use ckb_test_chain_utils::{always_success_cellbase, always_success_consensus};
use ckb_types::prelude::*;
use ckb_types::{
    core::{cell::resolve_transaction, BlockBuilder, BlockNumber, TransactionView},
    packed::Byte32,
};
use std::collections::HashSet;
use std::sync::Arc;

pub fn build_chain(tip: BlockNumber) -> (SyncShared, ChainController) {
    let (shared, table) = SharedBuilder::default()
        .consensus(always_success_consensus())
        .build()
        .unwrap();
    let chain_controller = {
        let chain_service = ChainService::new(shared.clone(), table);
        chain_service.start::<&str>(None)
    };
    generate_blocks(&shared, &chain_controller, tip);
    let sync_shared = SyncShared::new(shared);
    (sync_shared, chain_controller)
}

pub fn generate_blocks(
    shared: &Shared,
    chain_controller: &ChainController,
    target_tip: BlockNumber,
) {
    let snapshot = shared.snapshot();
    let parent_number = snapshot.tip_number();
    let mut parent_hash = snapshot.tip_header().hash();
    for _ in parent_number..target_tip {
        let block = inherit_block(shared, &parent_hash).build();
        parent_hash = block.header().hash();
        chain_controller
            .internal_process_block(Arc::new(block), Switch::DISABLE_ALL)
            .expect("processing block should be ok");
    }
}

pub fn inherit_block(shared: &Shared, parent_hash: &Byte32) -> BlockBuilder {
    let snapshot = shared.snapshot();
    let parent = snapshot.get_block(parent_hash).unwrap();
    let parent_epoch = snapshot.get_block_epoch(parent_hash).unwrap();
    let parent_number = parent.header().number();
    let epoch = snapshot
        .next_epoch_ext(snapshot.consensus(), &parent_epoch, &parent.header())
        .unwrap_or(parent_epoch);
    let cellbase = {
        let (_, reward) = snapshot.finalize_block_reward(&parent.header()).unwrap();
        always_success_cellbase(parent_number + 1, reward.total, snapshot.consensus())
    };
    let dao = {
        let resolved_cellbase = resolve_transaction(
            cellbase,
            &mut HashSet::new(),
            snapshot.as_ref(),
            snapshot.as_ref(),
        )
        .unwrap();
        DaoCalculator::new(shared.consensus(), snapshot.as_ref())
            .dao_field(&[resolved_cellbase], &parent.header())
            .unwrap()
    };

    BlockBuilder::default()
        .parent_hash(parent_hash.to_owned())
        .number((parent.header().number() + 1).pack())
        .timestamp((parent.header().timestamp() + 1).pack())
        .epoch(epoch.number_with_fraction(parent_number + 1).pack())
        .compact_target(epoch.compact_target().pack())
        .dao(dao)
        .transaction(inherit_cellbase(&snapshot, parent_number))
}

pub fn inherit_cellbase(snapshot: &Snapshot, parent_number: BlockNumber) -> TransactionView {
    let parent_header = {
        let parent_hash = snapshot
            .get_block_hash(parent_number)
            .expect("parent exist");
        snapshot
            .get_block_header(&parent_hash)
            .expect("parent exist")
    };
    let (_, reward) = snapshot.finalize_block_reward(&parent_header).unwrap();
    always_success_cellbase(parent_number + 1, reward.total, snapshot.consensus())
}
