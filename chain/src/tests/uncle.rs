use crate::tests::util::{MockChain, MockStore};
use crate::{chain::ChainService, switch::Switch};
use ckb_chain_spec::consensus::Consensus;
use ckb_shared::shared::SharedBuilder;
use ckb_store::ChainStore;
use std::sync::Arc;

#[test]
fn test_get_block_body_after_inserting() {
    let builder = SharedBuilder::default();
    let (shared, table) = builder.consensus(Consensus::default()).build().unwrap();
    let mut chain_service = ChainService::new(shared.clone(), table);
    let genesis = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    let parent = genesis;
    let mock_store = MockStore::new(&parent, shared.store());
    let mut fork1 = MockChain::new(parent.clone(), shared.consensus());
    let mut fork2 = MockChain::new(parent, shared.consensus());
    for _ in 0..4 {
        fork1.gen_empty_block_with_diff(100u64, &mock_store);
        fork2.gen_empty_block_with_diff(90u64, &mock_store);
    }

    for blk in fork1.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), Switch::DISABLE_ALL)
            .unwrap();
        let len = shared.snapshot().get_block_body(&blk.hash()).len();
        assert_eq!(len, 1, "[fork1] snapshot.get_block_body({})", blk.hash(),);
    }
    for blk in fork2.blocks() {
        chain_service
            .process_block(Arc::new(blk.clone()), Switch::DISABLE_ALL)
            .unwrap();
        let snapshot = shared.snapshot();
        assert!(snapshot.get_block_header(&blk.hash()).is_some());
        assert!(snapshot.get_block_uncles(&blk.hash()).is_some());
        assert!(snapshot.get_block_proposal_txs_ids(&blk.hash()).is_some());
        let len = snapshot.get_block_body(&blk.hash()).len();
        assert_eq!(len, 1, "[fork2] snapshot.get_block_body({})", blk.hash(),);
    }
}
