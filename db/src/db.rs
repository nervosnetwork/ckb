use crate::snapshot::RocksDBSnapshot;
use crate::transaction::RocksDBTransaction;
use crate::{Col, DBConfig, Error, Result};
use ckb_logger::{info, warn};
use rocksdb::ops::{Get, GetColumnFamilys, GetPinnedCF, IterateCF, OpenCF, Put, SetOptions};
use rocksdb::{
    ffi, ColumnFamily, DBPinnableSlice, IteratorMode, OptimisticTransactionDB,
    OptimisticTransactionOptions, Options, WriteOptions,
};
use std::sync::Arc;

// If any data format in database was changed, we have to update this constant manually.
//      - If the data can be migrated at startup automatically: update "x.y.z1" to "x.y.z2".
//      - If the data can be migrated manually: update "x.y1.z" to "x.y2.0".
//      - If the data can not be migrated: update "x1.y.z" to "x2.0.0".
pub(crate) const VERSION_KEY: &str = "db-version";
pub(crate) const VERSION_VALUE: &str = "0.1900.0";

pub struct RocksDB {
    pub(crate) inner: Arc<OptimisticTransactionDB>,
}

impl RocksDB {
    pub(crate) fn open_with_check(
        config: &DBConfig,
        columns: u32,
        ver_key: &str,
        ver_val: &str,
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
                            Error::DBError(format!(
                                "failed to open a new created database: {}",
                                err
                            ))
                        })?;
                    db.put(ver_key, ver_val).map_err(|err| {
                        Error::DBError(format!("failed to initiate the database: {}", err))
                    })?;
                    Ok(db)
                } else if err.as_ref().starts_with("Corruption:") {
                    warn!("Repairing the rocksdb since {} ...", err);
                    let mut repair_opts = Options::default();
                    repair_opts.create_if_missing(false);
                    repair_opts.create_missing_column_families(false);
                    OptimisticTransactionDB::repair(repair_opts, &config.path).map_err(|err| {
                        Error::DBError(format!("failed to repair the database: {}", err))
                    })?;
                    warn!("Opening the repaired rocksdb ...");
                    OptimisticTransactionDB::open_cf(&opts, &config.path, &cf_options).map_err(
                        |err| {
                            Error::DBError(format!("failed to open the repaired database: {}", err))
                        },
                    )
                } else {
                    Err(Error::DBError(format!(
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
                .map(|_| Error::DBError("failed to set database option".to_owned()))?;
        }

        let version_bytes = db
            .get(ver_key)
            .map_err(|err| {
                Error::DBError(format!("failed to check the version of database: {}", err))
            })?
            .ok_or_else(|| Error::DBError("version info about database is lost".to_owned()))?;
        let version_str = unsafe { ::std::str::from_utf8_unchecked(&version_bytes) };
        let version = semver::Version::parse(version_str)
            .map_err(|err| Error::DBError(format!("database version is malformed: {}", err)))?;
        let required_version = semver::Version::parse(ver_val).map_err(|err| {
            Error::DBError(format!("required database version is malformed: {}", err))
        })?;
        if required_version.major != version.major
            || required_version.minor != version.minor
            || required_version.patch < version.patch
        {
            Err(Error::DBError(format!(
                "the database version is not matched, require {} but it's {}",
                required_version, version
            )))?;
        } else if required_version.patch > version.patch {
            warn!(
                "Migrating the data from {} to {} ...",
                required_version, version
            );
            // Do data migration here.
            db.put(ver_key, ver_val).map_err(|err| {
                Error::DBError(format!("Failed to update database version: {}", err))
            })?;
        }

        Ok(RocksDB {
            inner: Arc::new(db),
        })
    }

    // TODO Change `panic(...)` to `Result<...>`
    pub fn open(config: &DBConfig, columns: u32) -> Self {
        Self::open_with_check(config, columns, VERSION_KEY, VERSION_VALUE)
            .unwrap_or_else(|err| panic!("{}", err))
    }

    pub fn open_with_error(config: &DBConfig, columns: u32) -> Result<Self> {
        Self::open_with_check(config, columns, VERSION_KEY, VERSION_VALUE)
    }

    pub fn open_tmp(columns: u32) -> Self {
        let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
        let config = DBConfig {
            path: tmp_dir.path().to_path_buf(),
            ..Default::default()
        };
        Self::open_with_check(&config, columns, VERSION_KEY, VERSION_VALUE)
            .unwrap_or_else(|err| panic!("{}", err))
    }

    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice>> {
        let cf = cf_handle(&self.inner, col)?;
        self.inner.get_pinned_cf(cf, &key).map_err(Into::into)
    }

    pub fn traverse<F>(&self, col: Col, mut callback: F) -> Result<()>
    where
        F: FnMut(&[u8], &[u8]) -> Result<()>,
    {
        let cf = cf_handle(&self.inner, col)?;
        let iter = self.inner.full_iterator_cf(cf, IteratorMode::Start)?;
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
}

pub(crate) fn cf_handle(db: &OptimisticTransactionDB, col: Col) -> Result<&ColumnFamily> {
    db.cf_handle(col)
        .ok_or_else(|| Error::DBError(format!("column {} not found", col)))
}

#[cfg(test)]
mod tests {
    use super::{DBConfig, Error, Result, RocksDB, VERSION_KEY, VERSION_VALUE};
    use std::collections::HashMap;
    use tempfile;

    fn setup_db(prefix: &str, columns: u32) -> RocksDB {
        setup_db_with_check(prefix, columns, VERSION_KEY, VERSION_VALUE).unwrap()
    }

    fn setup_db_with_check(
        prefix: &str,
        columns: u32,
        ver_key: &str,
        ver_val: &str,
    ) -> Result<RocksDB> {
        let tmp_dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };

        RocksDB::open_with_check(&config, columns, ver_key, ver_val)
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
            options: Some({
                let mut opts = HashMap::new();
                opts.insert("letsrock".to_owned(), "true".to_owned());
                opts
            }),
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
    fn test_version_is_not_matched() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_version_is_not_matched")
            .tempdir()
            .unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };
        let _ = RocksDB::open_with_check(&config, 1, VERSION_KEY, "0.1.0");
        let r = RocksDB::open_with_check(&config, 1, VERSION_KEY, "0.2.0");
        assert_eq!(
            r.err(),
            Some(Error::DBError(
                "the database version is not matched, require 0.2.0 but it's 0.1.0".to_owned()
            ))
        );
    }

    #[test]
    fn test_version_is_matched() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_version_is_matched")
            .tempdir()
            .unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };
        let _ = RocksDB::open_with_check(&config, 1, VERSION_KEY, VERSION_VALUE).unwrap();
        let _ = RocksDB::open_with_check(&config, 1, VERSION_KEY, VERSION_VALUE).unwrap();
    }
}
