use criterion::{criterion_group, BatchSize, BenchmarkId, Criterion};
use rand::prelude::SliceRandom;
use rand::thread_rng;

use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_sync::orphan_block_pool::OrphanBlockPool;
use ckb_types::core::{BlockBuilder, BlockView, HeaderView};
use ckb_types::prelude::*;

#[cfg(not(feature = "ci"))]
const BLOCKS_CNT: usize = 500 * CHUNK_SIZE;

#[cfg(feature = "ci")]
const BLOCKS_CNT: usize = CHUNK_SIZE;

const CHUNK_SIZE: usize = 2048;

/// test orphan block pool data structure and operation performance, focus on insert and remove
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
    BlockBuilder::default()
        .parent_hash(parent_header.hash())
        .number((parent_header.number() + 1).pack())
        .build()
}

/// 1000K blocks, divide into 500 groups, each group with 2048 blocks.
/// in each group, we shuffle the order of blocks and insert all blocks into pool except the 1st block of the group,
/// then call remove_blocks_by_parent to claim all blocks in the group.
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
            pool.insert(block.clone());
        }
        heads_index.push(start);
    }

    for index in heads_index {
        pool.remove_blocks_by_parent(&blocks[index].hash());
    }
    pool.remove_blocks_by_parent(&consensus.genesis_block().hash());
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("orphan_block_pool");

    group.bench_with_input(
        BenchmarkId::new("insert_remove", BLOCKS_CNT),
        &BLOCKS_CNT,
        |b, n_blocks| {
            b.iter_batched(
                || setup_chain(*n_blocks),
                |(pool, blocks, consensus)| {
                    test_sync(&pool, &blocks, &consensus);
                },
                BatchSize::PerIteration,
            )
        },
    );
}

criterion_group!(
    name = orphan_block_pool;
    config = Criterion::default().sample_size(10);
    targets = bench
);
