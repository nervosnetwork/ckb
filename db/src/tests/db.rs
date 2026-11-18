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
fn traversal_reports_sst_read_errors() {
    use crate::{DBIterator, IteratorMode};
    use rocksdb::ops::FlushCF;
    use std::{
        fs,
        io::{Read, Write},
    };

    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), 1);
    let mut batch = db.new_write_batch();
    batch.put("0", b"key", b"value").unwrap();
    db.write(&batch).unwrap();
    db.inner.flush_cf(db.generation().cf("0").unwrap()).unwrap();
    drop((batch, db));

    let sst = fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "sst"))
        .unwrap();
    // Damage a data block, leaving the table metadata and manifest readable.
    let mut byte = [0];
    fs::File::open(&sst).unwrap().read_exact(&mut byte).unwrap();
    byte[0] ^= 0x80;
    fs::OpenOptions::new()
        .write(true)
        .open(sst)
        .unwrap()
        .write_all(&byte)
        .unwrap();

    let db = RocksDB::open_in(directory.path(), 1);
    let mut iter = db.iter("0", IteratorMode::Start).unwrap();
    assert!(iter.next().is_none());
    assert!(iter.status().is_err());
    assert!(db.full_traverse("0", &mut |_, _| Ok(())).is_err());
    assert!(
        db.traverse("0", &mut |_, _| Ok(()), IteratorMode::Start, 10)
            .is_err()
    );
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
fn transaction_snapshot_requires_the_same_database() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(first.path(), 2);
    let other = RocksDB::open_in(second.path(), 2);
    let txn = db.transaction();
    let other_txn = other.transaction();
    let foreign = other_txn.get_snapshot();
    assert!(txn.get_for_update("0", b"key", &foreign).is_err());

    let same_db_txn = db.transaction();
    let snapshot = same_db_txn.get_snapshot();
    assert!(
        txn.get_for_update("0", b"key", &snapshot)
            .unwrap()
            .is_none()
    );
    txn.put("0", b"key", b"value").unwrap();
    txn.commit().unwrap();
    assert_eq!(
        db.get_pinned("0", b"key").unwrap().unwrap().as_ref(),
        b"value"
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

#[test]
fn missing_manifest_member_is_not_recreated_or_cleaned_up_on_open() {
    use ckb_db_schema::{
        COLUMNS,
        freezer::{SEGMENTS_KEY, Segments, segment_name},
    };
    let dir = tempfile::tempdir().unwrap();
    let config = DBConfig {
        path: dir.path().to_owned(),
        ..Default::default()
    };
    let db = RocksDB::open_with_check(&config, COLUMNS).unwrap();
    db.open_epoch_segment(0).unwrap();
    let missing = Segments::default().open(0).unwrap().open(16).unwrap();
    db.put_default(SEGMENTS_KEY, missing.encode().unwrap())
        .unwrap();
    drop(db);
    let before = rocksdb::DB::list_cf(&Default::default(), dir.path()).unwrap();
    assert!(before.contains(&segment_name(0)) && !before.contains(&segment_name(16)));
    assert!(
        RocksDB::open_with_check(&config, COLUMNS)
            .err()
            .unwrap()
            .to_string()
            .contains("published epoch segment 16 is missing")
    );
    let after = rocksdb::DB::list_cf(&Default::default(), dir.path()).unwrap();
    assert_eq!(after, before);
}

#[test]
fn nonempty_segment_with_lost_manifest_is_preserved_for_recovery() {
    use ckb_db_schema::{
        COLUMNS,
        freezer::{SEGMENTS_KEY, segment_name},
    };
    use rocksdb::ops::Delete;
    let dir = tempfile::tempdir().unwrap();
    let config = DBConfig {
        path: dir.path().into(),
        ..Default::default()
    };
    let db = RocksDB::open_with_check(&config, COLUMNS).unwrap();
    db.open_epoch_segment(0).unwrap();
    let tx = db.transaction();
    tx.put_segment(0, b"existing", b"rows").unwrap();
    tx.commit().unwrap();
    drop(tx);
    db.inner().delete(SEGMENTS_KEY).unwrap();
    drop(db);
    let err = RocksDB::open_with_check(&config, COLUMNS).err().unwrap();
    assert!(err.to_string().contains("nonempty epoch segment"));
    assert!(
        rocksdb::DB::list_cf(&Default::default(), dir.path())
            .unwrap()
            .contains(&segment_name(0))
    );
}

#[test]
fn conflicting_or_ambiguous_width_refuses_before_cleanup() {
    use ckb_db_schema::{
        COLUMNS,
        freezer::{SEGMENTS_KEY, segment_name},
    };
    use rocksdb::ops::{GetColumnFamilys, GetPinnedCF, OpenCF};
    let dir = tempfile::tempdir().unwrap();
    let mut config = DBConfig {
        path: dir.path().into(),
        freezer_epoch_width: Some(8),
        ..Default::default()
    };
    let db = RocksDB::open_with_check(&config, COLUMNS).unwrap();
    db.open_epoch_segment(0).unwrap();
    let tx = db.transaction();
    tx.put_segment(0, b"original", b"retired physical data")
        .unwrap();
    tx.commit().unwrap();
    drop(tx);
    db.open_epoch_segment(8).unwrap();
    // Durable unpublication followed by interruption before physical drop.
    let manifest = db.epoch_segments().retire(0).unwrap().encode().unwrap();
    db.put_default(SEGMENTS_KEY, &manifest).unwrap();
    drop(db);
    config.freezer_epoch_width = Some(16);
    assert!(RocksDB::open_with_check(&config, COLUMNS).is_err());
    assert!(RocksDB::prepare_for_bulk_load_with_width(dir.path(), COLUMNS, Some(16)).is_err());
    let names = rocksdb::DB::list_cf(&Default::default(), dir.path()).unwrap();
    let raw = rocksdb::DB::open_cf(&Default::default(), dir.path(), &names).unwrap();
    let cf = raw.cf_handle(&segment_name(0)).unwrap();
    assert_eq!(
        raw.get_pinned_cf(cf, b"original").unwrap().as_deref(),
        Some(b"retired physical data".as_slice())
    );
    use rocksdb::ops::Put;
    raw.put(
        SEGMENTS_KEY,
        [b"ROW1".as_slice(), &8u64.to_le_bytes()].concat(),
    )
    .unwrap();
    drop(raw);
    config.freezer_epoch_width = None;
    assert!(
        RocksDB::open_with_check(&config, COLUMNS)
            .err()
            .unwrap()
            .to_string()
            .contains("ROW1")
    );
    assert_eq!(
        rocksdb::DB::list_cf(&Default::default(), dir.path()).unwrap(),
        names
    );
}
