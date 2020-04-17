use crate::migration::Migrations;
use crate::snapshot::RocksDBSnapshot;
use crate::transaction::RocksDBTransaction;
use crate::{internal_error, Col, DBConfig, Result};
use ckb_logger::{info, warn};
use rocksdb::ops::{GetColumnFamilys, GetPinnedCF, GetPropertyCF, IterateCF, OpenCF, SetOptions};
use rocksdb::{
    ffi, ColumnFamily, DBPinnableSlice, IteratorMode, OptimisticTransactionDB,
    OptimisticTransactionOptions, Options, WriteOptions,
};
use std::sync::Arc;

pub const VERSION_KEY: &str = "db-version";

pub struct RocksDB {
    pub(crate) inner: Arc<OptimisticTransactionDB>,
}

impl RocksDB {
    pub(crate) fn open_with_check(
        config: &DBConfig,
        columns: u32,
        migrations: Migrations,
    ) -> Result<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(false);
        opts.create_missing_column_families(true);

        let cfnames: Vec<_> = (0..columns).map(|c| c.to_string()).collect();
        let cf_options: Vec<&str> = cfnames.iter().map(|n| n as &str).collect();

        let db =
            OptimisticTransactionDB::open_cf(&opts, &config.path, &cf_options).or_else(|err| {
                let err_str = err.as_ref();
                if err_str.starts_with("Invalid argument:")
                    && err_str.ends_with("does not exist (create_if_missing is false)")
                {
                    info!("Initialize a new database");
                    opts.create_if_missing(true);
                    let db = OptimisticTransactionDB::open_cf(&opts, &config.path, &cf_options)
                        .map_err(|err| {
                            internal_error(format!(
                                "failed to open a new created database: {}",
                                err
                            ))
                        })?;
                    Ok(db)
                } else if err.as_ref().starts_with("Corruption:") {
                    warn!("Repairing the rocksdb since {} ...", err);
                    let mut repair_opts = Options::default();
                    repair_opts.create_if_missing(false);
                    repair_opts.create_missing_column_families(false);
                    OptimisticTransactionDB::repair(repair_opts, &config.path).map_err(|err| {
                        internal_error(format!("failed to repair the database: {}", err))
                    })?;
                    warn!("Opening the repaired rocksdb ...");
                    OptimisticTransactionDB::open_cf(&opts, &config.path, &cf_options).map_err(
                        |err| {
                            internal_error(format!("failed to open the repaired database: {}", err))
                        },
                    )
                } else {
                    Err(internal_error(format!(
                        "failed to open the database: {}",
                        err
                    )))
                }
            })?;

        if let Some(db_opt) = config.options.as_ref() {
            let rocksdb_options: Vec<(&str, &str)> = db_opt
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            db.set_options(&rocksdb_options)
                .map_err(|_| internal_error("failed to set database option"))?;
        }

        let rocks_db = RocksDB {
            inner: Arc::new(db),
        };

        migrations.migrate(&rocks_db)?;

        Ok(rocks_db)
    }

    pub fn open(config: &DBConfig, columns: u32, migrations: Migrations) -> Self {
        Self::open_with_check(config, columns, migrations).unwrap_or_else(|err| panic!("{}", err))
    }

    pub fn open_tmp(columns: u32) -> Self {
        let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
        let config = DBConfig {
            path: tmp_dir.path().to_path_buf(),
            ..Default::default()
        };
        Self::open_with_check(&config, columns, Migrations::default())
            .unwrap_or_else(|err| panic!("{}", err))
    }

    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice>> {
        let cf = cf_handle(&self.inner, col)?;
        self.inner.get_pinned_cf(cf, &key).map_err(internal_error)
    }

    pub fn traverse<F>(&self, col: Col, mut callback: F) -> Result<()>
    where
        F: FnMut(&[u8], &[u8]) -> Result<()>,
    {
        let cf = cf_handle(&self.inner, col)?;
        let iter = self
            .inner
            .full_iterator_cf(cf, IteratorMode::Start)
            .map_err(internal_error)?;
        for (key, val) in iter {
            callback(&key, &val)?;
        }
        Ok(())
    }

    /// Set a snapshot at start of transaction by setting set_snapshot=true
    pub fn transaction(&self) -> RocksDBTransaction {
        let write_options = WriteOptions::default();
        let mut transaction_options = OptimisticTransactionOptions::new();
        transaction_options.set_snapshot(true);

        RocksDBTransaction {
            db: Arc::clone(&self.inner),
            inner: self.inner.transaction(&write_options, &transaction_options),
        }
    }

    pub fn get_snapshot(&self) -> RocksDBSnapshot {
        unsafe {
            let snapshot = ffi::rocksdb_create_snapshot(self.inner.base_db_ptr());
            RocksDBSnapshot::new(&self.inner, snapshot)
        }
    }

    pub fn property_value(&self, col: Col, name: &str) -> Result<Option<String>> {
        let cf = cf_handle(&self.inner, col)?;
        self.inner
            .property_value_cf(cf, name)
            .map_err(internal_error)
    }

    pub fn property_int_value(&self, col: Col, name: &str) -> Result<Option<u64>> {
        let cf = cf_handle(&self.inner, col)?;
        self.inner
            .property_int_value_cf(cf, name)
            .map_err(internal_error)
    }
}

pub(crate) fn cf_handle(db: &OptimisticTransactionDB, col: Col) -> Result<&ColumnFamily> {
    db.cf_handle(col)
        .ok_or_else(|| internal_error(format!("column {} not found", col)))
}

#[cfg(test)]
mod tests {
    use super::{DBConfig, Result, RocksDB, VERSION_KEY};
    use crate::migration::{DefaultMigration, Migration, Migrations};
    use rocksdb::ops::Get;
    use std::collections::HashMap;
    use tempfile;

    fn setup_db(prefix: &str, columns: u32) -> RocksDB {
        setup_db_with_check(prefix, columns).unwrap()
    }

    fn setup_db_with_check(prefix: &str, columns: u32) -> Result<RocksDB> {
        let tmp_dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };

        RocksDB::open_with_check(&config, columns, Migrations::default())
    }

    #[test]
    fn test_set_rocksdb_options() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_set_rocksdb_options")
            .tempdir()
            .unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            options: Some({
                let mut opts = HashMap::new();
                opts.insert("disable_auto_compactions".to_owned(), "true".to_owned());
                opts
            }),
        };
        RocksDB::open(&config, 2, Migrations::default()); // no panic
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
            options: Some({
                let mut opts = HashMap::new();
                opts.insert("letsrock".to_owned(), "true".to_owned());
                opts
            }),
        };
        RocksDB::open(&config, 2, Migrations::default()); // panic
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

        assert!(
            vec![0u8, 0, 0].as_slice() == db.get_pinned("0", &[0, 0]).unwrap().unwrap().as_ref()
        );
        assert!(db.get_pinned("0", &[1, 1]).unwrap().is_none());

        assert!(db.get_pinned("1", &[0, 0]).unwrap().is_none());
        assert!(
            vec![1u8, 1, 1].as_slice() == db.get_pinned("1", &[1, 1]).unwrap().unwrap().as_ref()
        );

        assert!(db.get_pinned("1", &[2]).unwrap().is_none());

        let mut r = HashMap::new();
        let callback = |k: &[u8], v: &[u8]| -> Result<()> {
            r.insert(k.to_vec(), v.to_vec());
            Ok(())
        };
        db.traverse("1", callback).unwrap();
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

    #[test]
    fn test_default_migration() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_default_migration")
            .tempdir()
            .unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };

        {
            let mut migrations = Migrations::default();
            migrations.add_migration(Box::new(DefaultMigration::new("20191116225943")));
            let r = RocksDB::open_with_check(&config, 1, migrations).unwrap();
            assert_eq!(
                b"20191116225943".to_vec(),
                r.inner.get(VERSION_KEY).unwrap().unwrap().to_vec()
            );
        }
        {
            let mut migrations = Migrations::default();
            migrations.add_migration(Box::new(DefaultMigration::new("20191116225943")));
            migrations.add_migration(Box::new(DefaultMigration::new("20191127101121")));
            let r = RocksDB::open_with_check(&config, 1, migrations).unwrap();
            assert_eq!(
                b"20191127101121".to_vec(),
                r.inner.get(VERSION_KEY).unwrap().unwrap().to_vec()
            );
        }
    }

    #[test]
    fn test_customized_migration() {
        struct CustomizedMigration;
        const COLUMN: &str = "0";
        const VERSION: &str = "20191127101121";

        impl Migration for CustomizedMigration {
            fn migrate(&self, db: &RocksDB) -> Result<()> {
                let txn = db.transaction();
                // append 1u8 to each value of column `0`
                let migration = |key: &[u8], value: &[u8]| -> Result<()> {
                    let mut new_value = value.to_vec();
                    new_value.push(1);
                    txn.put(COLUMN, key, &new_value)?;
                    Ok(())
                };
                db.traverse(COLUMN, migration)?;
                txn.commit()
            }

            fn version(&self) -> &str {
                VERSION
            }
        }

        let tmp_dir = tempfile::Builder::new()
            .prefix("test_customized_migration")
            .tempdir()
            .unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };

        {
            let mut migrations = Migrations::default();
            migrations.add_migration(Box::new(DefaultMigration::new("20191116225943")));
            let db = RocksDB::open_with_check(&config, 1, migrations).unwrap();
            let txn = db.transaction();
            txn.put(COLUMN, &[1, 1], &[1, 1, 1]).unwrap();
            txn.put(COLUMN, &[2, 2], &[2, 2, 2]).unwrap();
            txn.commit().unwrap();
        }
        {
            let mut migrations = Migrations::default();
            migrations.add_migration(Box::new(DefaultMigration::new("20191116225943")));
            migrations.add_migration(Box::new(CustomizedMigration));
            let db = RocksDB::open_with_check(&config, 1, migrations).unwrap();
            assert!(
                vec![1u8, 1, 1, 1].as_slice()
                    == db.get_pinned(COLUMN, &[1, 1]).unwrap().unwrap().as_ref()
            );
            assert!(
                vec![2u8, 2, 2, 1].as_slice()
                    == db.get_pinned(COLUMN, &[2, 2]).unwrap().unwrap().as_ref()
            );
            assert_eq!(
                VERSION.as_bytes(),
                db.inner
                    .get(VERSION_KEY)
                    .unwrap()
                    .unwrap()
                    .to_vec()
                    .as_slice()
            );
        }
    }
}
