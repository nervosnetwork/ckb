use crate::benchmarks::util::{gen_secp_block, new_secp_chain};
use ckb_store::{self, ChainStore};
use ckb_verification_traits::Switch;
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group};
use std::sync::Arc;

#[cfg(not(feature = "ci"))]
const SIZES: &[usize] = &[100usize, 200, 500, 1000];

#[cfg(feature = "ci")]
const SIZES: &[usize] = &[2usize];

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("process_block");

    // benchmark processing 20 blocks on main branch
    for txs_size in SIZES.iter() {
        group.bench_with_input(
            BenchmarkId::new("secp main_branch", txs_size),
            txs_size,
            |b, txs_size| {
                b.iter_batched(
                    || {
                        let chains = new_secp_chain(*txs_size, 2);
                        let (ref chain1, ref shared1) = chains.0[0];
                        let (ref chain2, ref shared2) = chains.0[1];
                        let mut blocks = vec![
                            shared1
                                .snapshot()
                                .get_block(&shared1.genesis_hash())
                                .unwrap(),
                        ];
                        let mut parent = blocks[0].clone();
                        (0..20).for_each(|_| {
                            let block = gen_secp_block(&mut blocks, &parent, shared2);
                            chain2
                                .blocking_process_block_with_switch(
                                    Arc::new(block.clone()),
                                    Switch::DISABLE_ALL,
                                )
                                .expect("process block OK");
                            parent = block;
                        });
                        (chain1.clone(), blocks)
                    },
                    |(chain, blocks)| {
                        blocks.into_iter().skip(1).for_each(|block| {
                            chain
                                .blocking_process_block_with_switch(
                                    Arc::new(block),
                                    Switch::DISABLE_EXTENSION,
                                )
                                .expect("process block OK");
                        });
                    },
                    BatchSize::PerIteration,
                )
            },
        );
    }

    // benchmark processing 2 blocks on side branch
    // 0 -> 1 -> 2 -> 3 -> 4 -> 5
    //           |
    //             -> 6 -> 7
    for txs_size in SIZES.iter() {
        group.bench_with_input(
            BenchmarkId::new("secp side_branch", txs_size),
            txs_size,
            |b, txs_size| {
                b.iter_batched(
                    || {
                        let chains = new_secp_chain(*txs_size, 3);
                        let (ref chain1, ref shared1) = chains.0[0];
                        let (ref chain2, ref shared2) = chains.0[1];
                        let (ref chain3, ref shared3) = chains.0[2];
                        let mut blocks = vec![
                            shared1
                                .snapshot()
                                .get_block(&shared1.genesis_hash())
                                .unwrap(),
                        ];
                        let mut parent = blocks[0].clone();
                        (0..5).for_each(|i| {
                            let block = gen_secp_block(&mut blocks, &parent, shared2);
                            chain2
                                .blocking_process_block_with_switch(
                                    Arc::new(block.clone()),
                                    Switch::DISABLE_ALL,
                                )
                                .expect("process block OK");
                            if i < 2 {
                                chain3
                                    .blocking_process_block_with_switch(
                                        Arc::new(block.clone()),
                                        Switch::DISABLE_ALL,
                                    )
                                    .expect("process block OK");
                            }
                            parent = block;
                        });
                        let mut parent = blocks[2].clone();
                        (0..2).for_each(|_| {
                            let block = gen_secp_block(&mut blocks, &parent, shared3);
                            chain3
                                .blocking_process_block_with_switch(
                                    Arc::new(block.clone()),
                                    Switch::DISABLE_ALL,
                                )
                                .expect("process block OK");
                            parent = block;
                        });
                        blocks
                            .clone()
                            .into_iter()
                            .skip(1)
                            .take(5)
                            .for_each(|block| {
                                chain1
                                    .blocking_process_block_with_switch(
                                        Arc::new(block),
                                        Switch::DISABLE_ALL,
                                    )
                                    .expect("process block OK");
                            });
                        (chain1.clone(), blocks)
                    },
                    |(chain, blocks)| {
                        blocks.into_iter().skip(6).for_each(|block| {
                            chain
                                .blocking_process_block(Arc::new(block))
                                .expect("process block OK");
                        });
                    },
                    BatchSize::PerIteration,
                )
            },
        );
    }

    // benchmark processing 4 blocks for switching fork
    // 0 -> 1 -> 2 -> 3 -> 4 -> 5
    //           |
    //             -> 6 -> 7 -> 8 -> 9
    for txs_size in SIZES.iter() {
        group.bench_with_input(
            BenchmarkId::new("secp switch_fork", txs_size),
            txs_size,
            |b, txs_size| {
                b.iter_batched(
                    || {
                        let chains = new_secp_chain(*txs_size, 3);
                        let (ref chain1, ref shared1) = chains.0[0];
                        let (ref chain2, ref shared2) = chains.0[1];
                        let (ref chain3, ref shared3) = chains.0[2];
                        let mut blocks = vec![
                            shared1
                                .snapshot()
                                .get_block(&shared1.genesis_hash())
                                .unwrap(),
                        ];
                        let mut parent = blocks[0].clone();
                        (0..5).for_each(|i| {
                            let block = gen_secp_block(&mut blocks, &parent, shared2);
                            let arc_block = Arc::new(block.clone());
                            chain2
                                .blocking_process_block_with_switch(
                                    Arc::clone(&arc_block),
                                    Switch::DISABLE_ALL,
                                )
                                .expect("process block OK");
                            if i < 2 {
                                chain3
                                    .blocking_process_block_with_switch(
                                        arc_block,
                                        Switch::DISABLE_ALL,
                                    )
                                    .expect("process block OK");
                            }
                            parent = block;
                        });
                        let mut parent = blocks[2].clone();
                        (0..4).for_each(|_| {
                            let block = gen_secp_block(&mut blocks, &parent, shared3);
                            chain3
                                .blocking_process_block_with_switch(
                                    Arc::new(block.clone()),
                                    Switch::DISABLE_ALL,
                                )
                                .expect("process block OK");
                            parent = block;
                        });
                        blocks
                            .clone()
                            .into_iter()
                            .skip(1)
                            .take(7)
                            .for_each(|block| {
                                chain1
                                    .blocking_process_block_with_switch(
                                        Arc::new(block),
                                        Switch::DISABLE_ALL,
                                    )
                                    .expect("process block OK");
                            });
                        (chain1.clone(), blocks)
                    },
                    |(chain, blocks)| {
                        blocks.into_iter().skip(8).for_each(|block| {
                            chain
                                .blocking_process_block_with_switch(
                                    Arc::new(block),
                                    Switch::DISABLE_EXTENSION,
                                )
                                .expect("process block OK");
                        });
                    },
                    BatchSize::PerIteration,
                )
            },
        );
    }
}

criterion_group!(
    name = process_block;
    config = Criterion::default().sample_size(10);
    targets = bench
);
