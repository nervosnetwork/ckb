use crate::DBWithTTL;

#[test]
fn ttl_shard_retirement_reclaims_wal() {
    use rocksdb::{
        ColumnFamilyDescriptor, Options, TTLOpenDescriptor,
        ops::{Flush, FlushWal, Get, OpenCF, Put},
    };

    let directory = tempfile::tempdir().unwrap();
    let mut options = Options::default();
    options.create_if_missing(true);
    options.create_missing_column_families(true);
    options.set_max_total_wal_size(1 << 20);
    options.set_write_buffer_size(64 << 20);
    let columns = ["default", "old"].map(|name| ColumnFamilyDescriptor::new(name, options.clone()));
    let mut db = DBWithTTL {
        inner: rocksdb::DBWithTTL::open_cf_descriptors_with_descriptor(
            &options,
            directory.path(),
            columns,
            TTLOpenDescriptor::by_default(100),
        )
        .unwrap(),
    };
    db.inner.put(b"marker", b"unchanged").unwrap();
    db.put("old", b"payload", vec![7; 2 << 20]).unwrap();
    db.inner.flush().unwrap();
    db.create_cf_with_ttl("current", 100).unwrap();
    db.drop_cf("old").unwrap();
    let value = vec![9; 256 << 10];
    for key in 0..32u8 {
        db.put("current", [key], &value).unwrap();
    }
    db.inner.flush_wal(false).unwrap();
    super::assert_wal_bounded(directory.path(), 3 << 20, || {
        db.inner.put(b"pressure-probe", []).unwrap();
    });
    drop(db);
    let db = DBWithTTL::open_cf(directory.path(), ["current"], 100).unwrap();
    assert_eq!(
        db.inner.get(b"marker").unwrap().unwrap().as_ref(),
        b"unchanged"
    );
    for key in 0..32u8 {
        assert_eq!(
            db.get_pinned("current", &[key]).unwrap().unwrap().as_ref(),
            value
        );
    }
}

#[test]
fn test_open_db_with_ttl() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_open_db_with_ttl")
        .tempdir()
        .unwrap();

    let db = DBWithTTL::open_cf(&tmp_dir, vec!["1"], 100);
    assert!(db.is_ok(), "{db:?}");
    let mut db = db.unwrap();

    for i in 0..1000u64 {
        db.put("1", i.to_le_bytes(), [2]).unwrap();
        assert_eq!(
            db.get_pinned("1", &i.to_le_bytes())
                .unwrap()
                .unwrap()
                .as_ref(),
            &[2]
        );
    }

    let estimate_num_keys = db.estimate_num_keys_cf("1").unwrap();
    assert!(estimate_num_keys.is_some());

    db.drop_cf("1").unwrap();
    let ret = db.get_pinned("1", &[1]);
    assert!(ret.is_err());
    let err_msg = format!("{:?}", ret.unwrap_err());
    assert!(err_msg.contains("column 1 not found"), "{}", err_msg);

    db.create_cf_with_ttl("1", 50).unwrap();
    assert!(db.get_pinned("1", &[1]).unwrap().is_none());
    db.put("1", [1], [3]).unwrap();
    assert_eq!(db.get_pinned("1", &[1]).unwrap().unwrap().as_ref(), &[3]);
}
