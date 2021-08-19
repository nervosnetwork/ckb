use criterion::{criterion_group, BatchSize, BenchmarkId, Criterion};
use rand::prelude::SliceRandom;
use rand::thread_rng;

use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_sync::orphan_block_pool::OrphanBlockPool;
use ckb_types::core::{BlockBuilder, BlockView, HeaderView};
use ckb_types::prelude::*;

#[cfg(not(feature = "ci"))]
const SIZE: usize = 1024 * 8;

#[cfg(feature = "ci")]
const SIZE: usize = 512;

const CHUNK_SIZE: usize = 256;

pub fn setup_chain(
    block_num: usize,
) -> (OrphanBlockPool, Vec<ckb_types::core::BlockView>, Consensus) {
    let consensus = ConsensusBuilder::default().build();
    let mut blocks = Vec::new();
    let mut parent = consensus.genesis_block().header();
    let pool = OrphanBlockPool::with_capacity(block_num);
    for _ in 0..block_num {
        let new_block = gen_block(&parent);
        blocks.push(new_block.clone());
        parent = new_block.header();
    }
    (pool, blocks, consensus)
}

fn gen_block(parent_header: &HeaderView) -> BlockView {
    let since_the_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    BlockBuilder::default()
        .parent_hash(parent_header.hash())
        .timestamp(since_the_epoch.pack())
        .number((parent_header.number() + 1).pack())
        .nonce((parent_header.nonce() + 1).pack())
        .build()
}

fn test_sync(pool: &OrphanBlockPool, blocks: &[ckb_types::core::BlockView], consensus: &Consensus) {
    let mut rng = thread_rng();
    let mut heads_index = vec![];

    for (index, _) in blocks.chunks(CHUNK_SIZE).enumerate() {
        let start = CHUNK_SIZE * index;
        let end = CHUNK_SIZE * (index + 1) - 1;
        let mut v: Vec<usize> = (start + 1..=end).collect();
        v.shuffle(&mut rng);
        for seq in v.iter() {
            let block = blocks.get(*seq).expect("seq wrong?");
            pool.insert(block.clone()).expect("insert error");
        }
        heads_index.push(start);
    }

    // index 0 doesn't in the orphan pool, so do nothing
    pool.remove_blocks_by_parent(&consensus.genesis_block().hash());

    for index in heads_index {
        pool.insert(blocks[index].clone()).expect("insert error");
        pool.remove_blocks_by_parent(&blocks[index].hash());
    }
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("orphan_block_pool");

    group.bench_with_input(BenchmarkId::new("remove", SIZE), &SIZE, |b, n_blocks| {
        b.iter_batched(
            || setup_chain(*n_blocks),
            |(pool, blocks, consensus)| {
                test_sync(&pool, &blocks, &consensus);
            },
            BatchSize::PerIteration,
        )
    });
}

criterion_group!(
    name = orphan_block_pool;
    config = Criterion::default().sample_size(10);
    targets = bench
);
