use crate::benchmarks::util::{gen_secp_block, new_secp_chain};
use ckb_merkle_mountain_range::util::MemMMR;
use ckb_store::{self, ChainStore};
use ckb_traits::chain_provider::ChainProvider;
use ckb_types::{packed::HeaderDigest, utilities::MergeHeaderDigest};
use criterion::{criterion_group, Criterion};
use std::sync::Arc;

#[cfg(not(feature = "ci"))]
const SIZES: &[usize] = &[100usize, 200, 500, 1000];

#[cfg(feature = "ci")]
const SIZES: &[usize] = &[2usize];

fn bench(c: &mut Criterion) {
    ckb_store::set_cache_enable(false);
    // benchmark processing 20 blocks on main branch
    c.bench_function_over_inputs(
        "secp main_branch",
        |b, txs_size| {
            b.iter_with_setup(
                || {
                    let chains = new_secp_chain(**txs_size, 2);
                    let (ref chain1, ref shared1) = chains.0[0];
                    let (ref chain2, ref shared2) = chains.0[1];
                    let mut mmr = MemMMR::<HeaderDigest, MergeHeaderDigest>::default();
                    let mut blocks =
                        vec![shared1.store().get_block(&shared1.genesis_hash()).unwrap()];
                    let mut parent = blocks[0].clone();
                    mmr.push(parent.header().into()).unwrap();
                    (0..20).for_each(|_| {
                        let chain_root = mmr.get_root().unwrap().hash();
                        let block = gen_secp_block(&mut blocks, &parent, shared2, chain_root);
                        chain2
                            .process_block(Arc::new(block.clone()), false)
                            .expect("process block OK");
                        parent = block;
                        mmr.push(parent.header().into()).unwrap();
                    });
                    (chain1.clone(), blocks)
                },
                |(chain, blocks)| {
                    blocks.into_iter().skip(1).for_each(|block| {
                        chain
                            .process_block(Arc::new(block), true)
                            .expect("process block OK");
                    });
                },
            )
        },
        SIZES,
    );

    // benchmark processing 2 blocks on side branch
    // 0 -> 1 -> 2 -> 3 -> 4 -> 5
    //           |
    //             -> 6 -> 7
    c.bench_function_over_inputs(
        "secp side_branch",
        |b, txs_size| {
            b.iter_with_setup(
                || {
                    let chains = new_secp_chain(**txs_size, 3);
                    let (ref chain1, ref shared1) = chains.0[0];
                    let (ref chain2, ref shared2) = chains.0[1];
                    let (ref chain3, ref shared3) = chains.0[2];
                    let mut mmr = MemMMR::<HeaderDigest, MergeHeaderDigest>::default();
                    let mut blocks =
                        vec![shared1.store().get_block(&shared1.genesis_hash()).unwrap()];
                    let mut parent = blocks[0].clone();
                    mmr.push(parent.header().into()).unwrap();
                    (0..5).for_each(|i| {
                        let chain_root = mmr.get_root().unwrap().hash();
                        let block = gen_secp_block(&mut blocks, &parent, &shared2, chain_root);
                        chain2
                            .process_block(Arc::new(block.clone()), false)
                            .expect("process block OK");
                        if i < 2 {
                            chain3
                                .process_block(Arc::new(block.clone()), false)
                                .expect("process block OK");
                        }
                        parent = block;
                        mmr.push(parent.header().into()).unwrap();
                    });
                    let mut parent = blocks[2].clone();
                    (0..2).for_each(|_| {
                        let chain_root = mmr.get_root().unwrap().hash();
                        let block = gen_secp_block(&mut blocks, &parent, &shared3, chain_root);
                        chain3
                            .process_block(Arc::new(block.clone()), false)
                            .expect("process block OK");
                        parent = block;
                        mmr.push(parent.header().into()).unwrap();
                    });
                    blocks
                        .clone()
                        .into_iter()
                        .skip(1)
                        .take(5)
                        .for_each(|block| {
                            chain1
                                .process_block(Arc::new(block), false)
                                .expect("process block OK");
                        });
                    (chain1.clone(), blocks)
                },
                |(chain, blocks)| {
                    blocks.into_iter().skip(6).for_each(|block| {
                        chain
                            .process_block(Arc::new(block), true)
                            .expect("process block OK");
                    });
                },
            )
        },
        SIZES,
    );

    // benchmark processing 4 blocks for switching fork
    // 0 -> 1 -> 2 -> 3 -> 4 -> 5
    //           |
    //             -> 6 -> 7 -> 8 -> 9
    c.bench_function_over_inputs(
        "secp switch_fork",
        |b, txs_size| {
            b.iter_with_setup(
                || {
                    let chains = new_secp_chain(**txs_size, 3);
                    let (ref chain1, ref shared1) = chains.0[0];
                    let (ref chain2, ref shared2) = chains.0[1];
                    let (ref chain3, ref shared3) = chains.0[2];
                    let mut mmr = MemMMR::<HeaderDigest, MergeHeaderDigest>::default();
                    let mut blocks =
                        vec![shared1.store().get_block(&shared1.genesis_hash()).unwrap()];
                    let mut parent = blocks[0].clone();
                    mmr.push(parent.header().into()).unwrap();
                    (0..5).for_each(|i| {
                        let chain_root = mmr.get_root().unwrap().hash();
                        let block = gen_secp_block(&mut blocks, &parent, &shared2, chain_root);
                        let arc_block = Arc::new(block.clone());
                        chain2
                            .process_block(Arc::clone(&arc_block), false)
                            .expect("process block OK");
                        if i < 2 {
                            chain3
                                .process_block(arc_block, false)
                                .expect("process block OK");
                        }
                        parent = block;
                        mmr.push(parent.header().into()).unwrap();
                    });
                    let mut parent = blocks[2].clone();
                    let mut mmr = {
                        let mut mmr = MemMMR::<HeaderDigest, MergeHeaderDigest>::default();
                        for block in &blocks[0..=2] {
                            mmr.push(block.header().into()).unwrap();
                        }
                        mmr
                    };
                    (0..4).for_each(|_| {
                        let chain_root = mmr.get_root().unwrap().hash();
                        let block = gen_secp_block(&mut blocks, &parent, &shared3, chain_root);
                        chain3
                            .process_block(Arc::new(block.clone()), false)
                            .expect("process block OK");
                        parent = block;
                        mmr.push(parent.header().into()).unwrap();
                    });
                    blocks
                        .clone()
                        .into_iter()
                        .skip(1)
                        .take(7)
                        .for_each(|block| {
                            chain1
                                .process_block(Arc::new(block), false)
                                .expect("process block OK");
                        });
                    (chain1.clone(), blocks)
                },
                |(chain, blocks)| {
                    blocks.into_iter().skip(8).for_each(|block| {
                        chain
                            .process_block(Arc::new(block), true)
                            .expect("process block OK");
                    });
                },
            )
        },
        SIZES,
    );
}

criterion_group!(
    name = process_block;
    config = Criterion::default().sample_size(10);
    targets = bench
);
