#[macro_use]
extern crate criterion;

use criterion::Criterion;

use bytes::Bytes;
use ckb_merkle_mountain_range::{util::MemStore, Error, MMRStore, Merge, Result, MMR};
use rand::{seq::SliceRandom, thread_rng};
use std::convert::TryFrom;

use blake2b_rs::{Blake2b, Blake2bBuilder};

fn new_blake2b() -> Blake2b {
    Blake2bBuilder::new(32).build()
}

#[derive(Eq, PartialEq, Clone, Debug, Default)]
struct NumberHash(pub Bytes);
impl TryFrom<u32> for NumberHash {
    type Error = Error;
    fn try_from(num: u32) -> Result<Self> {
        let mut hasher = new_blake2b();
        let mut hash = [0u8; 32];
        hasher.update(&num.to_le_bytes());
        hasher.finalize(&mut hash);
        Ok(NumberHash(hash.to_vec().into()))
    }
}

struct MergeNumberHash;

impl Merge for MergeNumberHash {
    type Item = NumberHash;
    fn merge(lhs: &Self::Item, rhs: &Self::Item) -> Self::Item {
        let mut hasher = new_blake2b();
        let mut hash = [0u8; 32];
        hasher.update(&lhs.0);
        hasher.update(&rhs.0);
        hasher.finalize(&mut hash);
        NumberHash(hash.to_vec().into())
    }
}

fn prepare_mmr(count: u32) -> (u64, MemStore<NumberHash>, Vec<u64>) {
    let store = MemStore::default();
    let mut mmr = MMR::<_, MergeNumberHash, _>::new(0, &store);
    let positions: Vec<u64> = (0u32..count)
        .map(|i| mmr.push(NumberHash::try_from(i).unwrap()).unwrap())
        .collect();
    let mmr_size = mmr.mmr_size();
    mmr.commit().expect("write to store");
    (mmr_size, store, positions)
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
        let (mmr_size, store, positions) = prepare_mmr(100_0000);
        let mmr = MMR::<_, MergeNumberHash, _>::new(mmr_size, &store);
        let mut rng = thread_rng();
        b.iter(|| mmr.gen_proof(*positions.choose(&mut rng).unwrap()));
    });

    c.bench_function("MMR verify", |b| {
        let (mmr_size, store, positions) = prepare_mmr(100_0000);
        let mmr = MMR::<_, MergeNumberHash, _>::new(mmr_size, &store);
        let mut rng = thread_rng();
        let root: NumberHash = mmr.get_root().unwrap();
        let proofs: Vec<_> = (0..10_000)
            .map(|_| {
                let pos = positions.choose(&mut rng).unwrap();
                let elem = (&store).get_elem(*pos).unwrap().unwrap();
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
    config = Criterion::default().sample_size(20);
    targets = bench
);
criterion_main!(benches);
