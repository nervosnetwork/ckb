#[macro_use]
extern crate criterion;

use criterion::Criterion;

use ckb_db::MemoryKeyValueDB;
use ckb_merkle_mountain_range::{tests_util::NumberHash, MMRStore, MMR};
use rand::{seq::SliceRandom, thread_rng};
use std::convert::TryFrom;
use std::sync::Arc;

type PrepareResult = (
    MMR<NumberHash, MemoryKeyValueDB>,
    Arc<MMRStore<NumberHash, MemoryKeyValueDB>>,
    Vec<u64>,
);

fn prepare_mmr(count: u32) -> PrepareResult {
    let mmr_store = Arc::new(MMRStore::new(MemoryKeyValueDB::open(1), 0));
    let mut mmr = MMR::new(0, Arc::clone(&mmr_store));
    let positions: Vec<u64> = (0u32..count)
        .map(|i| mmr.push(NumberHash::try_from(i).unwrap()).unwrap())
        .collect();
    (mmr, mmr_store, positions)
}

fn bench(c: &mut Criterion) {
    c.bench_function_over_inputs(
        "MMR insert",
        |b, &&size| {
            b.iter(|| prepare_mmr(size));
        },
        &[10_000, 100_000, 100_0000],
    );

    c.bench_function("MMR gen proof", |b| {
        let (mmr, _mmr_store, positions) = prepare_mmr(100_0000);
        let mut rng = thread_rng();
        b.iter(|| mmr.gen_proof(*positions.choose(&mut rng).unwrap()));
    });

    c.bench_function("MMR verify", |b| {
        let (mmr, mmr_store, positions) = prepare_mmr(100_0000);
        let mut rng = thread_rng();
        let root: NumberHash = mmr.get_root().unwrap().unwrap();
        let proofs: Vec<_> = (0..10_000)
            .map(|_| {
                let pos = positions.choose(&mut rng).unwrap();
                let elem = mmr_store.get_elem(*pos).unwrap().unwrap();
                let proof = mmr.gen_proof(*pos).unwrap();
                (pos, elem, proof)
            })
            .collect();
        b.iter(|| {
            let (pos, elem, proof) = proofs.choose(&mut rng).unwrap();
            proof.verify(root.clone(), **pos, elem.clone()).unwrap();
        });
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench
);
criterion_main!(benches);
