use ckb_app_config::DBConfig;
use std::collections::HashMap;

use crate::{Result, RocksDB};

fn setup_db(prefix: &str, columns: u32) -> RocksDB {
    setup_db_with_check(prefix, columns).unwrap()
}

fn setup_db_with_check(prefix: &str, columns: u32) -> Result<RocksDB> {
    let tmp_dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
    let config = DBConfig {
        path: tmp_dir.as_ref().to_path_buf(),
        ..Default::default()
    };

    RocksDB::open_with_check(&config, columns)
}

#[test]
fn test_set_rocksdb_options() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_set_rocksdb_options")
        .tempdir()
        .unwrap();
    let config = DBConfig {
        path: tmp_dir.as_ref().to_path_buf(),
        options: {
            let mut opts = HashMap::new();
            opts.insert("disable_auto_compactions".to_owned(), "true".to_owned());
            opts
        },
        ..Default::default()
    };
    RocksDB::open(&config, 2); // no panic
}

#[test]
fn test_set_rocksdb_options_empty() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_set_rocksdb_options_empty")
        .tempdir()
        .unwrap();
    let config = DBConfig {
        path: tmp_dir.as_ref().to_path_buf(),
        options: HashMap::new(),
        ..Default::default()
    };
    RocksDB::open(&config, 2); // no panic
}

#[test]
#[should_panic]
fn test_panic_on_invalid_rocksdb_options() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_panic_on_invalid_rocksdb_options")
        .tempdir()
        .unwrap();
    let config = DBConfig {
        path: tmp_dir.as_ref().to_path_buf(),
        options: {
            let mut opts = HashMap::new();
            opts.insert("letsrock".to_owned(), "true".to_owned());
            opts
        },
        ..Default::default()
    };
    RocksDB::open(&config, 2); // panic
}

#[test]
fn write_and_read() {
    let db = setup_db("write_and_read", 2);

    let txn = db.transaction();
    txn.put("0", &[0, 0], &[0, 0, 0]).unwrap();
    txn.put("1", &[1, 1], &[1, 1, 1]).unwrap();
    txn.put("1", &[2], &[1, 1, 1]).unwrap();
    txn.delete("1", &[2]).unwrap();
    txn.commit().unwrap();

    assert!(vec![0u8, 0, 0].as_slice() == db.get_pinned("0", &[0, 0]).unwrap().unwrap().as_ref());
    assert!(db.get_pinned("0", &[1, 1]).unwrap().is_none());

    assert!(db.get_pinned("1", &[0, 0]).unwrap().is_none());
    assert!(vec![1u8, 1, 1].as_slice() == db.get_pinned("1", &[1, 1]).unwrap().unwrap().as_ref());

    assert!(db.get_pinned("1", &[2]).unwrap().is_none());

    let mut r = HashMap::new();
    let mut callback = |k: &[u8], v: &[u8]| -> Result<()> {
        r.insert(k.to_vec(), v.to_vec());
        Ok(())
    };
    db.full_traverse("1", &mut callback).unwrap();
    assert!(r.len() == 1);
    assert_eq!(r.get(&vec![1, 1]), Some(&vec![1, 1, 1]));
}

#[test]
fn snapshot_isolation() {
    let db = setup_db("snapshot_isolation", 2);
    let snapshot = db.get_snapshot();
    let txn = db.transaction();
    txn.put("0", &[0, 0], &[5, 4, 3, 2]).unwrap();
    txn.put("1", &[1, 1], &[1, 2, 3, 4, 5]).unwrap();
    txn.commit().unwrap();

    assert!(snapshot.get_pinned("0", &[0, 0]).unwrap().is_none());
    assert!(snapshot.get_pinned("1", &[1, 1]).unwrap().is_none());
    let snapshot = db.get_snapshot();
    assert_eq!(
        snapshot.get_pinned("0", &[0, 0]).unwrap().unwrap().as_ref(),
        &[5, 4, 3, 2]
    );
    assert_eq!(
        snapshot.get_pinned("1", &[1, 1]).unwrap().unwrap().as_ref(),
        &[1, 2, 3, 4, 5]
    );
}

#[test]
fn write_and_partial_read() {
    let db = setup_db("write_and_partial_read", 2);

    let txn = db.transaction();
    txn.put("0", &[0, 0], &[5, 4, 3, 2]).unwrap();
    txn.put("1", &[1, 1], &[1, 2, 3, 4, 5]).unwrap();
    txn.commit().unwrap();

    let ret = db.get_pinned("1", &[1, 1]).unwrap().unwrap();

    assert!(vec![2u8, 3, 4].as_slice() == &ret.as_ref()[1..4]);
    assert!(db.get_pinned("1", &[0, 0]).unwrap().is_none());

    let ret = db.get_pinned("0", &[0, 0]).unwrap().unwrap();

    assert!(vec![4u8, 3, 2].as_slice() == &ret.as_ref()[1..4]);
}
