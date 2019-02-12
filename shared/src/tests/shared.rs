use crate::{
    block_median_time_context::BlockMedianTimeContext,
    shared::{Shared, SharedBuilder},
    store::{ChainKVStore, ChainStore},
};
use ckb_core::{block::BlockBuilder, header::HeaderBuilder};
use ckb_db::{kvdb::KeyValueDB, memorydb::MemoryKeyValueDB};
use numext_fixed_hash::H256;

fn new_shared() -> Shared<ChainKVStore<MemoryKeyValueDB>> {
    SharedBuilder::<MemoryKeyValueDB>::new().build()
}

fn insert_block_timestamps<T>(store: &ChainKVStore<T>, timestamps: &[u64]) -> Vec<H256>
where
    T: KeyValueDB + 'static,
{
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
    store
        .save_with_batch(|batch| {
            for b in blocks {
                store.insert_block(batch, &b);
            }
            Ok(())
        })
        .expect("insert blocks");
    hashes
}

#[test]
fn test_block_median_time() {
    let shared = new_shared();
    assert!(shared.block_median_time(&H256::zero()).is_none());
    let now = faketime::unix_time_as_millis();
    let block_hashes = insert_block_timestamps(shared.store(), &[now]);
    assert_eq!(
        shared
            .block_median_time(&block_hashes[0])
            .expect("median time"),
        now
    );
    let block_hashes = insert_block_timestamps(shared.store(), &(0..=22).collect::<Vec<_>>());
    assert_eq!(
        shared
            .block_median_time(&block_hashes.last().expect("last"))
            .expect("median time"),
        17
    );
}
