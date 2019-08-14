use crate::SyncSharedState;
use ckb_chain::chain::{ChainController, ChainService};

use ckb_dao::DaoCalculator;
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_test_chain_utils::{always_success_cellbase, always_success_consensus};
use ckb_traits::ChainProvider;
use ckb_types::prelude::*;
use ckb_types::{
    core::{cell::resolve_transaction, BlockBuilder, BlockNumber, TransactionView},
    packed::Byte32,
};
use std::collections::HashSet;
use std::sync::Arc;

pub fn build_chain(tip: BlockNumber) -> (SyncSharedState, ChainController) {
    let shared = SharedBuilder::default()
        .consensus(always_success_consensus())
        .build()
        .unwrap();
    let chain_controller = {
        let notify_controller = NotifyService::default().start::<&str>(None);
        let chain_service = ChainService::new(shared.clone(), notify_controller);
        chain_service.start::<&str>(None)
    };
    generate_blocks(&shared, &chain_controller, tip);
    let sync_shared_state = SyncSharedState::new(shared);
    (sync_shared_state, chain_controller)
}

pub fn generate_blocks(
    shared: &Shared,
    chain_controller: &ChainController,
    target_tip: BlockNumber,
) {
    let parent_number = shared.lock_chain_state().tip_number();
    let mut parent_hash = shared.lock_chain_state().tip_hash().clone();
    for _block_number in parent_number + 1..=target_tip {
        let block = inherit_block(shared, &parent_hash).build();
        parent_hash = block.header().hash().to_owned();
        chain_controller
            .process_block(Arc::new(block), false)
            .expect("processing block should be ok");
    }
}

pub fn inherit_block(shared: &Shared, parent_hash: &Byte32) -> BlockBuilder {
    let parent = shared.store().get_block(parent_hash).unwrap();
    let parent_epoch = shared.store().get_block_epoch(parent_hash).unwrap();
    let parent_number = parent.header().number();
    let epoch = shared
        .next_epoch_ext(&parent_epoch, &parent.header())
        .unwrap_or(parent_epoch);
    let cellbase = {
        let (_, reward) = shared.finalize_block_reward(&parent.header()).unwrap();
        always_success_cellbase(parent_number + 1, reward.total)
    };
    let dao = {
        let chain_state = shared.lock_chain_state();
        let resolved_cellbase =
            resolve_transaction(&cellbase, &mut HashSet::new(), &*chain_state, &*chain_state)
                .unwrap();
        DaoCalculator::new(shared.consensus(), shared.store())
            .dao_field(&[resolved_cellbase], &parent.header())
            .unwrap()
    };

    BlockBuilder::default()
        .parent_hash(parent_hash.to_owned())
        .number((parent.header().number() + 1).pack())
        .timestamp((parent.header().timestamp() + 1).pack())
        .epoch(epoch.number().pack())
        .difficulty(epoch.difficulty().pack())
        .dao(dao.pack())
        .transaction(inherit_cellbase(shared, parent_number))
}

pub fn inherit_cellbase(shared: &Shared, parent_number: BlockNumber) -> TransactionView {
    let parent_header = {
        let chain = shared.lock_chain_state();
        let parent_hash = chain
            .store()
            .get_block_hash(parent_number)
            .expect("parent exist");
        chain
            .store()
            .get_block_header(&parent_hash)
            .expect("parent exist")
    };
    let (_, reward) = shared.finalize_block_reward(&parent_header).unwrap();
    always_success_cellbase(parent_number + 1, reward.total)
}
