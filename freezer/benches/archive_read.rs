use ckb_freezer::{Freezer, FreezerFilesBuilder};
use ckb_types::{
    core::{BlockBuilder, TransactionBuilder},
    packed,
    prelude::*,
};
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use rocksdb::{DB, prelude::*};

const RECORDS: u64 = 256;
const RECORD_SIZE: usize = 64 * 1024;

fn payload(number: u64, length: usize) -> Vec<u8> {
    let mut bytes = vec![0; length];
    let mut state = number + 1;
    for byte in &mut bytes[length / 2..] {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
    bytes
}

fn block() -> packed::Block {
    BlockBuilder::default()
        .transactions((0..16u32).map(|index| {
            TransactionBuilder::default()
                .version(index)
                .witness(packed::Bytes::from(payload(index as u64, 4096)))
                .build()
        }))
        .build()
        .data()
}

fn archive_read(criterion: &mut Criterion) {
    let archive_dir = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let mut archive = FreezerFilesBuilder::new(archive_dir.path().to_path_buf())
        .build()
        .unwrap();
    let db = DB::open_default(db_dir.path()).unwrap();
    for number in 1..=RECORDS {
        let bytes = payload(number, RECORD_SIZE);
        archive.append(number, &bytes).unwrap();
        db.put(number.to_le_bytes(), bytes).unwrap();
    }
    let block = block();
    for number in RECORDS + 1..=RECORDS * 2 {
        archive.append(number, block.as_slice()).unwrap();
    }
    archive.sync_all().unwrap();
    drop(archive);
    db.flush().unwrap();
    let freezer = Freezer::open_in(archive_dir.path()).unwrap();

    let mut number = 0;
    criterion.bench_function("freezer/retrieve/64KiB", |bench| {
        bench.iter(|| {
            number = number % RECORDS + 1;
            black_box(freezer.retrieve(number).unwrap().unwrap())
        })
    });
    number = 0;
    criterion.bench_function("rocksdb/get/64KiB", |bench| {
        bench.iter(|| {
            number = number % RECORDS + 1;
            black_box(db.get(number.to_le_bytes()).unwrap().unwrap())
        })
    });
    let hash = block.calc_header_hash();
    number = 0;
    criterion.bench_function("freezer/transaction/16x4KiB", |bench| {
        bench.iter(|| {
            number = number % RECORDS + 1;
            let block = freezer
                .read_block(RECORDS + number, &hash)
                .unwrap()
                .unwrap();
            black_box(block.transaction(7).unwrap().unwrap())
        })
    });
}

criterion_group!(benches, archive_read);
criterion_main!(benches);
