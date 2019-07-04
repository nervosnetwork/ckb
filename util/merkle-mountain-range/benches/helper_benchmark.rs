#[macro_use]
extern crate criterion;
use criterion::Criterion;

use ckb_merkle_mountain_range::{leaf_index_to_mmr_size, leaf_index_to_pos};

use rand::{thread_rng, Rng};

fn bench(c: &mut Criterion) {
    c.bench_function("left_index_to_pos", |b| {
        let mut rng = thread_rng();
        b.iter(|| {
            let leaf_index = rng.gen_range(50_000_000_000, 70_000_000_000);
            leaf_index_to_pos(leaf_index);
        });
    });

    c.bench_function("left_index_to_mmr_size", |b| {
        let mut rng = thread_rng();
        b.iter(|| {
            let leaf_index = rng.gen_range(50_000_000_000, 70_000_000_000);
            leaf_index_to_mmr_size(leaf_index);
        });
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
