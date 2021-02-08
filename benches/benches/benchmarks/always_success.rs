use crate::benchmarks::util::{gen_always_success_block, new_always_success_chain};
use ckb_chain::switch::Switch;
use ckb_store::{self, ChainStore};
use criterion::{criterion_group, BatchSize, BenchmarkId, Criterion};
use std::sync::Arc;

#[cfg(not(feature = "ci"))]
const SIZES: &[usize] = &[100usize, 200, 500, 1000];

#[cfg(feature = "ci")]
const SIZES: &[usize] = &[5usize];

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("process_block");

    // benchmark processing 20 blocks on main branch
    for txs_size in SIZES.iter() {
        group.bench_with_input(
            BenchmarkId::new("always_success main_branch", txs_size),
            txs_size,
            |b, i| {
                b.iter_batched(
                    || {
                        let chains = new_always_success_chain(*i, 2);
                        let (ref chain1, ref shared1) = chains.0[0];
                        let (ref chain2, ref shared2) = chains.0[1];
                        let mut blocks = vec![shared1
                            .snapshot()
                            .get_block(&shared1.genesis_hash())
                            .unwrap()];
                        let mut parent = blocks[0].clone();
                        (0..20).for_each(|_| {
                            let block = gen_always_success_block(&mut blocks, &parent, shared2);
                            chain2
                                .internal_process_block(
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
                                .process_block(Arc::new(block))
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
            BenchmarkId::new("always_success side_branch", txs_size),
            txs_size,
            |b, i| {
                b.iter_batched(
                    || {
                        let chains = new_always_success_chain(*i, 3);
                        let (ref chain1, ref shared1) = chains.0[0];
                        let (ref chain2, ref shared2) = chains.0[1];
                        let (ref chain3, ref shared3) = chains.0[2];
                        let mut blocks = vec![shared1
                            .snapshot()
                            .get_block(&shared1.genesis_hash())
                            .unwrap()];
                        let mut parent = blocks[0].clone();
                        (0..5).for_each(|i| {
                            let block = gen_always_success_block(&mut blocks, &parent, &shared2);
                            chain2
                                .internal_process_block(
                                    Arc::new(block.clone()),
                                    Switch::DISABLE_ALL,
                                )
                                .expect("process block OK");
                            if i < 2 {
                                chain3
                                    .internal_process_block(
                                        Arc::new(block.clone()),
                                        Switch::DISABLE_ALL,
                                    )
                                    .expect("process block OK");
                            }
                            parent = block;
                        });
                        let mut parent = blocks[2].clone();
                        (0..2).for_each(|_| {
                            let block = gen_always_success_block(&mut blocks, &parent, &shared3);
                            chain3
                                .internal_process_block(
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
                                    .internal_process_block(Arc::new(block), Switch::DISABLE_ALL)
                                    .expect("process block OK");
                            });
                        (chain1.clone(), blocks)
                    },
                    |(chain, blocks)| {
                        blocks.into_iter().skip(6).for_each(|block| {
                            chain
                                .process_block(Arc::new(block))
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
            BenchmarkId::new("always_success switch_fork", txs_size),
            txs_size,
            |b, i| {
                b.iter_batched(
                    || {
                        let chains = new_always_success_chain(*i, 3);
                        let (ref chain1, ref shared1) = chains.0[0];
                        let (ref chain2, ref shared2) = chains.0[1];
                        let (ref chain3, ref shared3) = chains.0[2];
                        let mut blocks = vec![shared1
                            .snapshot()
                            .get_block(&shared1.genesis_hash())
                            .unwrap()];
                        let mut parent = blocks[0].clone();
                        (0..5).for_each(|i| {
                            let block = gen_always_success_block(&mut blocks, &parent, &shared2);
                            let arc_block = Arc::new(block.clone());
                            chain2
                                .internal_process_block(Arc::clone(&arc_block), Switch::DISABLE_ALL)
                                .expect("process block OK");
                            if i < 2 {
                                chain3
                                    .internal_process_block(arc_block, Switch::DISABLE_ALL)
                                    .expect("process block OK");
                            }
                            parent = block;
                        });
                        let mut parent = blocks[2].clone();
                        (0..4).for_each(|_| {
                            let block = gen_always_success_block(&mut blocks, &parent, &shared3);
                            chain3
                                .internal_process_block(
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
                                    .internal_process_block(Arc::new(block), Switch::DISABLE_ALL)
                                    .expect("process block OK");
                            });
                        (chain1.clone(), blocks)
                    },
                    |(chain, blocks)| {
                        blocks.into_iter().skip(8).for_each(|block| {
                            chain
                                .process_block(Arc::new(block))
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
