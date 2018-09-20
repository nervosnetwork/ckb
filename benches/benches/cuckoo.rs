#[macro_use]
extern crate criterion;
extern crate ckb_chain;

use ckb_chain::pow::Cuckoo;
use criterion::Criterion;

const TESTSET: [([u8; 80], [u32; 6]); 3] = [
    (
        [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x1c, 0, 0, 0,
        ],
        [0, 1, 2, 4, 5, 6],
    ),
    (
        [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x36, 0, 0, 0,
        ],
        [0, 1, 2, 3, 4, 7],
    ),
    (
        [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xf6, 0, 0, 0,
        ],
        [0, 1, 2, 4, 5, 7],
    ),
];

fn bench(c: &mut Criterion) {
    c.bench_function("bench_solve", |b| {
        let cuckoo = Cuckoo::new(16, 8, 6);
        b.iter(|| {
            for _ in 0..100 {
                for (message, _) in TESTSET.iter() {
                    cuckoo.solve(message).unwrap();
                }
            }
        })
    });

    c.bench_function("bench_verify", |b| {
        let cuckoo = Cuckoo::new(16, 8, 6);
        b.iter(|| {
            for _ in 0..100 {
                for (message, proof) in TESTSET.iter() {
                    cuckoo.verify(message, proof);
                }
            }
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
