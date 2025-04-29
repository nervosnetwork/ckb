use crate::SyncShared;
use ckb_chain::{ChainController, build_chain_services};
use ckb_dao::DaoCalculator;
use ckb_reward_calculator::RewardCalculator;
use ckb_shared::{ChainServicesBuilder, Shared, SharedBuilder, Snapshot};
use ckb_store::ChainStore;
use ckb_test_chain_utils::{always_success_cellbase, always_success_consensus};
use ckb_types::prelude::*;
use ckb_types::{
    core::{BlockBuilder, BlockNumber, TransactionView, cell::resolve_transaction},
    packed::Byte32,
};
use ckb_verification_traits::Switch;
use std::collections::HashSet;
use std::sync::Arc;
use std::thread::JoinHandle;

pub fn build_chain(tip: BlockNumber) -> (SyncShared, ChainServiceScope) {
    let (shared, mut pack) = SharedBuilder::with_temp_db()
        .consensus(always_success_consensus())
        .build()
        .unwrap();
    let chain_scope = ChainServiceScope::new(pack.take_chain_services_builder());
    generate_blocks(&shared, chain_scope.chain_controller(), tip);
    let sync_shared = SyncShared::new(shared, Default::default(), pack.take_relay_tx_receiver());
    (sync_shared, chain_scope)
}

// This structure restricts the scope of chain service, and forces chain
// service threads to terminate before dropping the structure.
// The content of this struct will always be present, the reason we
// wrap them in an option, is that we will need to consume them in
// Drop trait impl of this struct.
pub struct ChainServiceScope(Option<(ChainController, JoinHandle<()>)>);

impl ChainServiceScope {
    pub fn new(builder: ChainServicesBuilder) -> Self {
        let (controller, join_handle) = build_chain_services(builder);
        Self(Some((controller, join_handle)))
    }

    pub fn chain_controller(&self) -> &ChainController {
        &self.0.as_ref().unwrap().0
    }
}

impl Drop for ChainServiceScope {
    fn drop(&mut self) {
        let (controller, join_handle) = self.0.take().unwrap();
        drop(controller);
        let _ = join_handle.join();
    }
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
            .blocking_process_block_with_switch(Arc::new(block), Switch::DISABLE_ALL)
            .expect("processing block should be ok");
    }
}

pub fn inherit_block(shared: &Shared, parent_hash: &Byte32) -> BlockBuilder {
    let snapshot = shared.snapshot();
    let parent = snapshot.get_block(parent_hash).unwrap();
    let parent_number = parent.header().number();
    let epoch = snapshot
        .consensus()
        .next_epoch_ext(&parent.header(), &snapshot.borrow_as_data_loader())
        .unwrap()
        .epoch();
    let cellbase = inherit_cellbase(&snapshot, parent_number);
    let dao = {
        let resolved_cellbase = resolve_transaction(
            cellbase.clone(),
            &mut HashSet::new(),
            snapshot.as_ref(),
            snapshot.as_ref(),
        )
        .unwrap();
        let data_loader = snapshot.borrow_as_data_loader();
        DaoCalculator::new(shared.consensus(), &data_loader)
            .dao_field([resolved_cellbase].iter(), &parent.header())
            .unwrap()
    };

    let chain_root = shared
        .snapshot()
        .chain_root_mmr(parent_number)
        .get_root()
        .expect("chain root_mmr");
    let bytes = chain_root.calc_mmr_hash().as_bytes().pack();

    BlockBuilder::default()
        .parent_hash(parent_hash.to_owned())
        .number((parent.header().number() + 1).pack())
        .timestamp((parent.header().timestamp() + 1).pack())
        .epoch(epoch.number_with_fraction(parent_number + 1).pack())
        .compact_target(epoch.compact_target().pack())
        .dao(dao)
        .transaction(cellbase)
        .extension(Some(bytes))
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
    let (_, reward) = RewardCalculator::new(snapshot.consensus(), snapshot)
        .block_reward_to_finalize(&parent_header)
        .unwrap();
    always_success_cellbase(parent_number + 1, reward.total, snapshot.consensus())
}
