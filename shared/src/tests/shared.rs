use crate::shared::{Shared, SharedBuilder};
use ckb_core::{block::BlockBuilder, header::HeaderBuilder};
use ckb_db::{KeyValueDB, MemoryKeyValueDB};
use ckb_store::{ChainKVStore, ChainStore, StoreBatch};
use ckb_traits::chain_provider::ChainProvider;
use ckb_traits::BlockMedianTimeContext;

fn new_shared() -> Shared<ChainKVStore<MemoryKeyValueDB>> {
    SharedBuilder::<MemoryKeyValueDB>::new().build().unwrap()
}

fn insert_block_timestamps<T>(store: &ChainKVStore<T>, timestamps: &[u64])
where
    T: KeyValueDB,
{
    let mut blocks = Vec::with_capacity(timestamps.len());
    let tip_header = store.get_tip_header().expect("tip");
    let mut parent_hash = tip_header.hash().to_owned();
    let mut parent_number = tip_header.number();
    for timestamp in timestamps {
        let header = HeaderBuilder::default()
            .timestamp(*timestamp)
            .parent_hash(parent_hash.clone())
            .number(parent_number + 1)
            .build();
        parent_hash = header.hash().to_owned();
        parent_number += 1;
        blocks.push(BlockBuilder::default().header(header).build());
    }
    let mut batch = store.new_batch().unwrap();
    for b in blocks {
        batch.insert_block(&b).unwrap();
        batch.attach_block(&b).unwrap();
    }
    batch.commit().unwrap();
}

#[test]
fn test_block_median_time() {
    let shared = new_shared();
    let chain_state = shared.chain_state().lock();
    assert_eq!((&*chain_state).block_median_time(0), Some(0));
    let now = faketime::unix_time_as_millis();
    insert_block_timestamps(shared.store(), &[now]);
    assert_eq!(
        (&*chain_state).block_median_time(1).expect("median time"),
        now
    );
    let timestamps = (1..=22).collect::<Vec<_>>();
    insert_block_timestamps(shared.store(), &timestamps);
    assert_eq!(
        (&*chain_state)
            .block_median_time(*timestamps.last().expect("last"))
            .expect("median time"),
        17
    );
}
