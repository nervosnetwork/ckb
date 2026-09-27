use crate::{UnverifiedBlock, verify::ConsumeUnverifiedBlockProcessor};
use ckb_channel::bounded;
use ckb_shared::{HeaderIndex, SharedBuilder};
use ckb_test_chain_utils::{MockChain, MockStore};
use ckb_types::{U256, packed::Byte32};
use ckb_verification_traits::Switch;
use dashmap::DashSet;
use std::sync::Arc;

#[test]
fn successful_verification_restores_progress_after_an_earlier_failure_reset() {
    let (shared, mut package) = SharedBuilder::with_temp_db().build().unwrap();
    let builder = package.take_chain_services_builder();
    // This test drives the real verifier directly, without starting the pool.
    drop(package);
    let mut processor = ConsumeUnverifiedBlockProcessor {
        shared: shared.clone(),
        is_pending_verify: Arc::new(DashSet::new()),
        proposal_table: builder.proposal_table,
    };
    let parent = shared.consensus().genesis_block().header();
    let store = MockStore::new(&parent, shared.store());
    let mut chain = MockChain::new(parent.clone(), shared.consensus());
    chain.gen_empty_block_with_diff(40u64, &store);
    let valid = chain.blocks().last().unwrap().clone();
    let invalid = valid.as_advanced_builder().dao(Byte32::zero()).build();
    assert_ne!(invalid.hash(), valid.hash());
    let transaction = shared.store().begin_transaction();
    transaction.insert_block(&invalid).unwrap();
    transaction.insert_block(&valid).unwrap();
    transaction.commit().unwrap();

    // Both candidates have arrived before verification starts. Publishing
    // before enqueueing cannot prevent the earlier failure from resetting this.
    shared.set_unverified_tip(HeaderIndex::new(valid.number(), valid.hash(), U256::zero()));
    processor.consume_unverified_blocks(UnverifiedBlock {
        block: Arc::new(invalid),
        parent_header: parent.clone(),
        switch: Some(Switch::DISABLE_ALL & !Switch::DISABLE_DAOHEADER),
        verify_callback: Some(Box::new(|result| assert!(result.is_err()))),
    });
    assert_eq!(shared.get_unverified_tip().hash(), parent.hash());

    let (completed, completion) = bounded(1);
    let observer = shared.clone();
    let valid_hash = valid.hash();
    processor.consume_unverified_blocks(UnverifiedBlock {
        block: Arc::new(valid),
        parent_header: parent,
        switch: Some(Switch::DISABLE_ALL),
        verify_callback: Some(Box::new(move |result| {
            assert!(result.unwrap());
            completed.send(observer.get_unverified_tip()).unwrap();
        })),
    });
    let snapshot = shared.snapshot();
    assert_eq!(snapshot.tip_hash(), valid_hash);
    assert_eq!(
        completion.try_recv().unwrap(),
        HeaderIndex::new(
            snapshot.tip_number(),
            snapshot.tip_hash(),
            snapshot.total_difficulty().clone(),
        )
    );
}
