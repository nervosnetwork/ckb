use crate::chain::ChainService;
use crate::tests::util::{MockChain, MockStore};
use ckb_chain_spec::consensus::Consensus;
use ckb_shared::SharedBuilder;
use ckb_store::ChainStore;
use ckb_verification_traits::Switch;
use std::sync::Arc;

#[test]
fn test_truncate() {
    let builder = SharedBuilder::with_temp_db();

    let (shared, mut pack) = builder.consensus(Consensus::default()).build().unwrap();
    let mut chain_service = ChainService::new(shared.clone(), pack.take_proposal_table());

    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let mock_store = MockStore::new(&genesis, shared.store());
    let mut mock = MockChain::new(genesis, shared.consensus());

    for _ in 0..10 {
        mock.gen_empty_block_with_diff(40u64, &mock_store);
    }

    for blk in mock.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), Switch::DISABLE_ALL)
            .unwrap();
    }

    let target = shared.snapshot().tip_header().clone();

    for _ in 0..10 {
        mock.gen_empty_block_with_diff(40u64, &mock_store);
    }

    for blk in mock.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), Switch::DISABLE_ALL)
            .unwrap();
    }

    chain_service.truncate(&target.hash()).unwrap();

    assert_eq!(shared.snapshot().tip_header(), &target);
}
