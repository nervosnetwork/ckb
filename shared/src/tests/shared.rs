use crate::{
    shared::{Shared, SharedBuilder},
    store::{ChainKVStore, ChainStore, StoreBatch},
};
use ckb_core::{block::BlockBuilder, header::HeaderBuilder};
use ckb_db::{KeyValueDB, MemoryKeyValueDB};
use ckb_traits::BlockMedianTimeContext;
use numext_fixed_hash::H256;

fn new_shared() -> Shared<ChainKVStore<MemoryKeyValueDB>> {
    SharedBuilder::<MemoryKeyValueDB>::new().build()
}

fn insert_block_timestamps<T: KeyValueDB>(
    store: &ChainKVStore<T>,
    timestamps: &[u64],
) -> Vec<H256> {
    let mut blocks = Vec::with_capacity(timestamps.len());
    let mut hashes = Vec::with_capacity(timestamps.len());
    let mut parent_hash = H256::zero();
    for timestamp in timestamps {
        let header = HeaderBuilder::default()
            .timestamp(*timestamp)
            .parent_hash(parent_hash.clone())
            .build();
        parent_hash = header.hash();
        hashes.push(parent_hash.clone());
        blocks.push(BlockBuilder::default().header(header).build());
    }
    let mut batch = store.new_batch().unwrap();
    for b in blocks {
        batch.insert_block(&b).unwrap();
    }
    batch.commit().unwrap();
    hashes
}

#[test]
fn test_block_median_time() {
    let shared = new_shared();
    let chain_state = shared.chain_state().lock();
    assert!((&*chain_state).block_median_time(&H256::zero()).is_none());
    let now = faketime::unix_time_as_millis();
    let block_hashes = insert_block_timestamps(shared.store(), &[now]);
    assert_eq!(
        (&*chain_state)
            .block_median_time(&block_hashes[0])
            .expect("median time"),
        now
    );
    let block_hashes = insert_block_timestamps(shared.store(), &(0..=22).collect::<Vec<_>>());
    assert_eq!(
        (&*chain_state)
            .block_median_time(&block_hashes.last().expect("last"))
            .expect("median time"),
        17
    );
}
