use crate::tests::util::start_tx_pool;
use crate::{ChainController, ChainServiceScope};
use ckb_shared::{Shared, SharedBuilder};
use ckb_test_chain_utils::{MockChain, MockStore};
use ckb_verification_traits::Switch;
use std::sync::Arc;

#[test]
fn test_truncate() {
    let (shared, mut pack) = SharedBuilder::with_temp_db().build().unwrap();
    let _tx_pool = start_tx_pool(&shared, &mut pack);
    let chain = ChainServiceScope::new(pack.take_chain_services_builder());
    assert_truncate(chain.chain_controller(), &shared);
}

#[test]
fn test_truncate_without_tx_pool() {
    let (shared, pack) = SharedBuilder::with_temp_db().build().unwrap();
    let chain = ChainServiceScope::new(pack.into_chain_services_builder());
    assert_truncate(chain.chain_controller(), &shared);
}

fn assert_truncate(chain_controller: &ChainController, shared: &Shared) {
    let genesis = shared.consensus().genesis_block().header();

    let mock_store = MockStore::new(&genesis, shared.store());
    let mut mock = MockChain::new(genesis, shared.consensus());

    for _ in 0..10 {
        mock.gen_empty_block_with_diff(40u64, &mock_store);
    }

    for blk in mock.blocks() {
        chain_controller
            .blocking_process_block_with_switch(Arc::new(blk.clone()), Switch::DISABLE_ALL)
            .unwrap();
    }

    let target = shared.snapshot().tip_header().clone();

    for _ in 0..10 {
        mock.gen_empty_block_with_diff(40u64, &mock_store);
    }

    for blk in mock.blocks() {
        chain_controller
            .blocking_process_block_with_switch(Arc::new(blk.clone()), Switch::DISABLE_ALL)
            .unwrap();
    }

    chain_controller.truncate(target.hash()).unwrap();

    assert_eq!(shared.snapshot().tip_header(), &target);
}
