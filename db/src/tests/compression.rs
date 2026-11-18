use crate::{DBWithTTL, ReadOnlyDB, RocksDB};
use ckb_app_config::DBConfig;
use ckb_db_schema::COLUMNS;
use rocksdb::{
    ColumnFamilyDescriptor, DBCompressionType, FullOptions, OptimisticTransactionDB, Options,
    TTLOpenDescriptor, prelude::*,
};
use std::path::Path;

fn assert_compression(path: &Path, expected: &str) {
    let options = std::fs::read_dir(path)
        .unwrap()
        .map(Result::unwrap)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("OPTIONS-")
        })
        .max()
        .unwrap();
    let text = std::fs::read_to_string(&options).unwrap();
    let codecs: Vec<_> = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("compression="))
        .collect();
    assert!(!codecs.is_empty());
    assert!(codecs.iter().all(|codec| *codec == expected), "{codecs:?}");
    FullOptions::load_from_file(options, None, false).unwrap();
}

fn snappy_options() -> Options {
    let mut options = Options::default();
    options.create_if_missing(true);
    options.create_missing_column_families(true);
    options.set_compression_type(DBCompressionType::Snappy);
    options
}

#[test]
fn main_database_uses_lz4_and_reopens_legacy_snappy_ssts() {
    for legacy in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let value = vec![b'x'; 8192];
        if legacy {
            let options = snappy_options();
            let columns = std::iter::once("default".to_owned())
                .chain((0..COLUMNS - 1).map(|column| column.to_string()))
                .map(|name| ColumnFamilyDescriptor::new(name, options.clone()));
            let db =
                OptimisticTransactionDB::open_cf_descriptors(&options, directory.path(), columns)
                    .unwrap();
            let column = db.cf_handle("0").unwrap();
            db.put_cf(column, b"legacy", &value).unwrap();
            db.compact_range_cf(column, None::<&[u8]>, None::<&[u8]>);
            drop(db);
            assert_compression(directory.path(), "kSnappyCompression");
        }
        let config = DBConfig {
            path: directory.path().to_path_buf(),
            ..Default::default()
        };
        let db = RocksDB::open_with_check(&config, COLUMNS).unwrap();
        if legacy {
            assert_eq!(
                db.get_pinned("0", b"legacy").unwrap().unwrap().as_ref(),
                value
            );
        }
        let transaction = db.transaction();
        transaction.put("0", b"new", &value).unwrap();
        transaction.commit().unwrap();
        drop(db);
        assert_compression(directory.path(), "kLZ4Compression");
        let read_only = ReadOnlyDB::open_cf(directory.path(), ["0"])
            .unwrap()
            .unwrap();
        assert_eq!(
            read_only.get_pinned("0", b"new").unwrap().unwrap().as_ref(),
            value
        );
        if legacy {
            assert_eq!(
                read_only
                    .get_pinned("0", b"legacy")
                    .unwrap()
                    .unwrap()
                    .as_ref(),
                value
            );
        }
    }
}

#[test]
fn main_database_preserves_explicit_compression_options() {
    let directory = tempfile::tempdir().unwrap();
    let options_file = directory.path().join("db-options");
    std::fs::write(
        &options_file,
        "[DBOptions]\n[CFOptions \"default\"]\ncompression=kSnappyCompression\n",
    )
    .unwrap();
    let config = DBConfig {
        path: directory.path().join("db"),
        options_file: Some(options_file),
        ..Default::default()
    };
    drop(RocksDB::open_with_check(&config, COLUMNS).unwrap());
    assert_compression(&config.path, "kSnappyCompression");
}

#[test]
fn ttl_database_uses_lz4_and_reopens_legacy_snappy_ssts() {
    for legacy in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let value = vec![b'x'; 8192];
        if legacy {
            let options = snappy_options();
            let columns = ["default", "0", "1"]
                .map(|name| ColumnFamilyDescriptor::new(name, options.clone()));
            let db = rocksdb::DBWithTTL::open_cf_descriptors_with_descriptor(
                &options,
                directory.path(),
                columns,
                TTLOpenDescriptor::by_default(-1),
            )
            .unwrap();
            let column = db.cf_handle("0").unwrap();
            db.put_cf(column, b"legacy", &value).unwrap();
            db.compact_range_cf(column, None::<&[u8]>, None::<&[u8]>);
            drop(db);
            assert_compression(directory.path(), "kSnappyCompression");
        }
        let mut db = DBWithTTL::open_cf(directory.path(), ["0", "1"], -1).unwrap();
        if legacy {
            assert_eq!(
                db.get_pinned("0", b"legacy").unwrap().unwrap().as_ref(),
                value
            );
        }
        // recent-reject also recreates shards when its count limit is reached.
        db.drop_cf("1").unwrap();
        db.create_cf_with_ttl("1", -1).unwrap();
        db.put("1", b"new", &value).unwrap();
        db.inner.compact_range_cf(
            db.inner.cf_handle("1").unwrap(),
            None::<&[u8]>,
            None::<&[u8]>,
        );
        drop(db);
        assert_compression(directory.path(), "kLZ4Compression");
        let db = DBWithTTL::open_cf(directory.path(), ["0", "1"], -1).unwrap();
        assert_eq!(db.get_pinned("1", b"new").unwrap().unwrap().as_ref(), value);
        if legacy {
            assert_eq!(
                db.get_pinned("0", b"legacy").unwrap().unwrap().as_ref(),
                value
            );
        }
    }
}
