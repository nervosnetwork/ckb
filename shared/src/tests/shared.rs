use crate::shared::{Shared, SharedBuilder};
use ckb_store::{ChainDB, ChainStore};
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use ckb_types::{
    core::{BlockBuilder, HeaderBuilder},
    prelude::*,
};

fn new_shared() -> Shared {
    SharedBuilder::default().build().unwrap()
}

fn insert_block_timestamps(store: &ChainDB, timestamps: &[u64]) {
    let mut blocks = Vec::with_capacity(timestamps.len());
    let tip_header = store.get_tip_header().expect("tip");
    let mut parent_hash = tip_header.hash().to_owned();
    let mut parent_number = tip_header.number();
    for timestamp in timestamps {
        let header = HeaderBuilder::default()
            .timestamp(timestamp.pack())
            .parent_hash(parent_hash.clone())
            .number((parent_number + 1).pack())
            .build();
        parent_hash = header.hash().to_owned();
        parent_number += 1;
        blocks.push(BlockBuilder::default().header(header).build());
    }
    let txn = store.begin_transaction();
    for b in blocks {
        txn.insert_block(&b).unwrap();
        txn.attach_block(&b).unwrap();
    }
    txn.commit().unwrap();
}

#[test]
fn test_block_median_time() {
    let shared = new_shared();
    let chain_state = shared.lock_chain_state();
    let hash = shared.store().get_block_hash(0).unwrap();
    assert_eq!((&*chain_state).block_median_time(&hash), 0);
    let now = faketime::unix_time_as_millis();
    insert_block_timestamps(shared.store(), &[now]);
    let hash = shared.store().get_block_hash(1).unwrap();
    assert_eq!((&*chain_state).block_median_time(&hash), now);
    let timestamps = (1..=22).collect::<Vec<_>>();
    insert_block_timestamps(shared.store(), &timestamps);
    let block_number = *timestamps.last().expect("last");
    let hash = shared.store().get_block_hash(block_number).unwrap();
    assert_eq!((&*chain_state).block_median_time(&hash), 17);
}
