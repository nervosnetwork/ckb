use canonical_serializer::{CanonicalSerialize, CanonicalSerializer};
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::{Bytes, Capacity};
use criterion::{criterion_group, criterion_main, Benchmark, Criterion, Throughput};
use numext_fixed_hash::H256;
use occupied_capacity::capacity_bytes;
use rand::{thread_rng, Rng};

fn gen_tx() -> Transaction {
    fn gen_hash() -> H256 {
        let mut rng = thread_rng();
        let mut buf = [0u8; 32];
        rng.fill(&mut buf);
        buf.into()
    }
    fn gen_bytes(len: usize) -> Bytes {
        let mut rng = thread_rng();
        let mut buf = Vec::new();
        buf.resize(len, 0);
        rng.fill(buf.as_mut_slice());
        buf.into()
    }
    TransactionBuilder::default()
        .output(CellOutput::new(
            capacity_bytes!(50_000),
            gen_bytes(256),
            Script::new(vec![gen_bytes(256)], gen_hash()),
            None,
        ))
        .output(CellOutput::new(
            capacity_bytes!(50_000),
            gen_bytes(256),
            Script::new(vec![gen_bytes(256)], gen_hash()),
            None,
        ))
        .input(CellInput::new(OutPoint::new_cell(gen_hash(), 0), 0))
        .input(CellInput::new(OutPoint::new_cell(gen_hash(), 0), 0))
        .dep(OutPoint::new_cell(gen_hash(), 0))
        .witness(vec![gen_bytes(65), gen_bytes(65)])
        .build()
}

fn bench(c: &mut Criterion) {
    c.bench(
        "canonical serialize txs",
        Benchmark::new("serialize txs", |b| {
            let tx = gen_tx();
            b.iter_with_setup(
                || Vec::with_capacity(2000),
                |mut buf| {
                    let mut serializer = CanonicalSerializer::new(&mut buf);
                    tx.serialize(&mut serializer).expect("canonical serialize");
                },
            )
        })
        .throughput(Throughput::Elements(1u32)),
    );
}

criterion_group!(benches, bench);
criterion_main!(benches);
