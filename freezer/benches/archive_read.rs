//! Read-path diagnostics, not a ChainStore, RPC or cold-device benchmark.
#![allow(missing_docs)] // Criterion generates a public benchmark entry point.

use ckb_freezer::{Freezer, FreezerFilesBuilder};
use ckb_types::{
    core::{BlockBuilder, TransactionBuilder},
    packed,
    prelude::*,
};
use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use rocksdb::{BlockBasedOptions, Cache, DB, DBCompressionType, Options, ReadOptions, prelude::*};
use sha2::{Digest, Sha256};

const CACHE_BYTES: usize = 32 * 1024 * 1024;
const TRANSACTION: usize = 7;

fn random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn payload(seed: u64, length: usize) -> Vec<u8> {
    let mut bytes = vec![0; length];
    let mut state = seed + 1;
    for byte in &mut bytes[length / 2..] {
        *byte = random(&mut state) as u8;
    }
    bytes
}

fn block(number: u64) -> packed::Block {
    BlockBuilder::default()
        .number(number)
        .transactions((0..16u32).map(|index| {
            TransactionBuilder::default()
                .version(index)
                .witness(packed::Bytes::from(payload(
                    number * 16 + index as u64,
                    4096,
                )))
                .build()
        }))
        .extension(Some(payload(number, 128).into()))
        .build()
        .data()
}

#[derive(Clone, Copy)]
enum Field {
    Block,
    Transaction,
    Extension,
}

impl Field {
    fn name(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Transaction => "transaction",
            Self::Extension => "extension",
        }
    }

    fn key(self, number: u64) -> [u8; 9] {
        let mut key = [self as u8; 9];
        key[1..].copy_from_slice(&number.to_be_bytes());
        key
    }
}

fn archive_read(criterion: &mut Criterion) {
    let records = std::env::var("CKB_FREEZER_BENCH_RECORDS")
        .map(|value| value.parse::<u64>().expect("positive record count"))
        .unwrap_or(256);
    assert!(records > 0);
    let reverse = std::env::var_os("CKB_FREEZER_BENCH_REVERSE").is_some();
    let archive_dir = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let mut archive = FreezerFilesBuilder::new(archive_dir.path().to_path_buf())
        .build()
        .unwrap();
    let cache = Cache::new_lru_cache(CACHE_BYTES);
    let mut table = BlockBasedOptions::default();
    table.set_block_cache(&cache);
    let mut options = Options::default();
    options.create_if_missing(true);
    options.set_compression_type(DBCompressionType::Lz4);
    options.set_block_based_table_factory(&table);
    let db = DB::open(&options, db_dir.path()).unwrap();
    let mut hashes = Vec::new();
    let mut digest = Sha256::new();
    for number in 1..=records {
        let block = block(number);
        archive.append(number, block.as_slice()).unwrap();
        db.put(Field::Block.key(number), block.as_slice()).unwrap();
        db.put(
            Field::Transaction.key(number),
            block.transactions().get(TRANSACTION).unwrap().as_slice(),
        )
        .unwrap();
        db.put(
            Field::Extension.key(number),
            block.extension().unwrap().as_slice(),
        )
        .unwrap();
        hashes.push(block.calc_header_hash());
        digest.update(block.as_slice());
    }
    archive.sync_all().unwrap();
    drop(archive);
    db.flush().unwrap();
    drop(db); // Ensure reads cannot be served from the population memtable.
    let freezer = Freezer::open_in(archive_dir.path()).unwrap();
    let read_options = ReadOptions::default();

    // Verify all projections outside the timed loops. The fixture is a direct
    // value comparison; it deliberately excludes CKB's hash/index lookups.
    let db = DB::open(&options, db_dir.path()).unwrap();
    for (offset, hash) in hashes.iter().enumerate() {
        let number = offset as u64 + 1;
        let bytes = freezer.retrieve(number).unwrap().unwrap();
        let fields = freezer.read_block(number, hash).unwrap().unwrap();
        assert_eq!(
            bytes.as_slice(),
            db.get(Field::Block.key(number)).unwrap().unwrap().as_ref()
        );
        assert_eq!(
            fields.transaction(TRANSACTION).unwrap().unwrap().as_slice(),
            db.get(Field::Transaction.key(number))
                .unwrap()
                .unwrap()
                .as_ref()
        );
        assert_eq!(
            fields.extension().unwrap().unwrap().as_slice(),
            db.get(Field::Extension.key(number))
                .unwrap()
                .unwrap()
                .as_ref()
        );
    }
    drop(db);
    eprintln!(
        "dataset: records={records}, bytes_per_block={}, sha256={:x}, rocksdb_cache={CACHE_BYTES}, reverse={reverse}, os_cache=warm",
        block(1).as_slice().len(),
        digest.finalize()
    );

    let sequential: Vec<_> = (1..=records).collect();
    let mut shuffled = sequential.clone();
    let mut seed = 0x6f72_6465_725f_6531;
    for index in (1..shuffled.len()).rev() {
        let other = random(&mut seed) as usize % (index + 1);
        shuffled.swap(index, other);
    }
    // Separate decoded-block-cache and SST-read diagnostics. Neither evicts
    // the OS page cache. Freezer retains its default bounded decode cache.
    for rocksdb_cache in [true, false] {
        if !rocksdb_cache {
            table.disable_cache();
            options.set_block_based_table_factory(&table);
        }
        let db = DB::open(&options, db_dir.path()).unwrap();
        let cache_name = if rocksdb_cache {
            "block-cache"
        } else {
            "sst-read"
        };
        for (pattern, order) in [("sequential", &sequential), ("random", &shuffled)] {
            let mut group =
                criterion.benchmark_group(format!("reads/{records}/{cache_name}/{pattern}"));
            for field in [Field::Block, Field::Transaction, Field::Extension] {
                let backends = if reverse {
                    [false, true]
                } else {
                    [true, false]
                };
                for archived in backends {
                    let name = if archived { "freezer" } else { "rocksdb" };
                    let mut cursor = 0;
                    group.bench_function(BenchmarkId::new(name, field.name()), |bench| {
                        bench.iter(|| {
                            let number = order[cursor];
                            cursor = (cursor + 1) % order.len();
                            if archived {
                                match field {
                                    Field::Block => {
                                        black_box(freezer.retrieve(number).unwrap().unwrap()).len()
                                    }
                                    Field::Transaction => {
                                        let block = freezer
                                            .read_block(number, &hashes[number as usize - 1])
                                            .unwrap()
                                            .unwrap();
                                        black_box(block.transaction(TRANSACTION).unwrap().unwrap())
                                            .as_slice()
                                            .len()
                                    }
                                    Field::Extension => {
                                        let block = freezer
                                            .read_block(number, &hashes[number as usize - 1])
                                            .unwrap()
                                            .unwrap();
                                        black_box(block.extension().unwrap().unwrap())
                                            .as_slice()
                                            .len()
                                    }
                                }
                            } else {
                                black_box(
                                    db.get_opt(field.key(number), &read_options)
                                        .unwrap()
                                        .unwrap(),
                                )
                                .len()
                            }
                        })
                    });
                }
            }
            group.finish();
        }
    }
}

criterion_group!(benches, archive_read);
criterion_main!(benches);
