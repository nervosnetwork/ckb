use crate::{
    Col, DBConfig, DbBatch, Direction, Error, IterableKeyValueDB, KeyValueDB, KeyValueIteratorItem,
    Result,
};
use ckb_logger::{info, warn};
use rocksdb::{
    ColumnFamily, Direction as RdbDirection, Error as RdbError, IteratorMode, Options, WriteBatch,
    DB,
};
use std::ops::Range;
use std::sync::Arc;

// If any data format in database was changed, we have to update this constant manually.
//      - If the data can be migrated at startup automatically: update "x.y.z1" to "x.y.z2".
//      - If the data can be migrated manually: update "x.y1.z" to "x.y2.0".
//      - If the data can not be migrated: update "x1.y.z" to "x2.0.0".
pub(crate) const VERSION_KEY: &str = "db-version";
pub(crate) const VERSION_VALUE: &str = "0.1501.0";

pub struct RocksDB {
    inner: Arc<DB>,
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

        let db = DB::open_cf(&opts, &config.path, &cf_options).or_else(|err| {
            let err_str = err.as_ref();
            if err_str.starts_with("Invalid argument:")
                && err_str.ends_with("does not exist (create_if_missing is false)")
            {
                info!("Initialize a new database");
                opts.create_if_missing(true);
                let db = DB::open_cf(&opts, &config.path, &cf_options).map_err(|err| {
                    Error::DBError(format!("failed to open a new created database: {}", err))
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
                DB::repair(repair_opts, &config.path).map_err(|err| {
                    Error::DBError(format!("failed to repair the database: {}", err))
                })?;
                warn!("Opening the repaired rocksdb ...");
                DB::open_cf(&opts, &config.path, &cf_options).map_err(|err| {
                    Error::DBError(format!("failed to open the repaired database: {}", err))
                })
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
}

fn cf_handle(db: &DB, col: Col) -> Result<ColumnFamily> {
    db.cf_handle(&col.to_string())
        .ok_or_else(|| Error::DBError(format!("column {} not found", col)))
}

impl KeyValueDB for RocksDB {
    type Batch = RocksdbBatch;

    fn read(&self, col: Col, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let cf = cf_handle(&self.inner, col)?;
        self.inner
            .get_cf(cf, &key)
            .map(|v| v.map(|vi| vi.to_vec()))
            .map_err(Into::into)
    }

    fn partial_read(&self, col: Col, key: &[u8], range: &Range<usize>) -> Result<Option<Vec<u8>>> {
        let cf = cf_handle(&self.inner, col)?;
        self.inner
            .get_pinned_cf(cf, &key)
            .map(|v| v.and_then(|vi| vi.get(range.start..range.end).map(|slice| slice.to_vec())))
            .map_err(Into::into)
    }

    fn process_read<F, Ret>(&self, col: Col, key: &[u8], process: F) -> Result<Option<Ret>>
    where
        F: FnOnce(&[u8]) -> Result<Option<Ret>>,
    {
        let cf = cf_handle(&self.inner, col)?;
        if let Some(slice) = self.inner.get_pinned_cf(cf, &key)? {
            process(&slice)
        } else {
            Ok(None)
        }
    }

    fn traverse<F>(&self, col: Col, mut callback: F) -> Result<()>
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

    fn batch(&self) -> Result<Self::Batch> {
        Ok(Self::Batch {
            db: Arc::clone(&self.inner),
            wb: WriteBatch::default(),
        })
    }
}

impl IterableKeyValueDB for RocksDB {
    fn iter<'a>(
        &'a self,
        col: Col,
        from_key: &'a [u8],
        direction: Direction,
    ) -> Result<Box<Iterator<Item = KeyValueIteratorItem> + 'a>> {
        let cf = cf_handle(&self.inner, col)?;
        let iter_direction = match direction {
            Direction::Forward => RdbDirection::Forward,
            Direction::Reverse => RdbDirection::Reverse,
        };
        let mode = IteratorMode::From(from_key, iter_direction);
        self.inner
            .iterator_cf(cf, mode)
            .map(|iter| Box::new(iter) as Box<_>)
            .map_err(Into::into)
    }
}

pub struct RocksdbBatch {
    db: Arc<DB>,
    wb: WriteBatch,
}

impl DbBatch for RocksdbBatch {
    fn insert(&mut self, col: Col, key: &[u8], value: &[u8]) -> Result<()> {
        let cf = cf_handle(&self.db, col)?;
        self.wb.put_cf(cf, key, value)?;
        Ok(())
    }

    fn delete(&mut self, col: Col, key: &[u8]) -> Result<()> {
        let cf = cf_handle(&self.db, col)?;
        self.wb.delete_cf(cf, &key)?;
        Ok(())
    }

    fn commit(self) -> Result<()> {
        self.db.write(self.wb)?;
        Ok(())
    }
}

impl From<RdbError> for Error {
    fn from(err: RdbError) -> Error {
        Error::DBError(err.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

        let mut batch = db.batch().unwrap();
        batch.insert(0, &[0, 0], &[0, 0, 0]).unwrap();
        batch.insert(1, &[1, 1], &[1, 1, 1]).unwrap();
        batch.insert(1, &[2], &[1, 1, 1]).unwrap();
        batch.delete(1, &[2]).unwrap();
        batch.commit().unwrap();

        assert_eq!(Some(vec![0, 0, 0]), db.read(0, &[0, 0]).unwrap());
        assert_eq!(None, db.read(0, &[1, 1]).unwrap());

        assert_eq!(None, db.read(1, &[0, 0]).unwrap());
        assert_eq!(Some(vec![1, 1, 1]), db.read(1, &[1, 1]).unwrap());

        assert_eq!(None, db.read(1, &[2]).unwrap());

        let mut r = HashMap::new();
        let callback = |k: &[u8], v: &[u8]| -> Result<()> {
            r.insert(k.to_vec(), v.to_vec());
            Ok(())
        };
        db.traverse(1, callback).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r.get(&vec![1, 1]), Some(&vec![1, 1, 1]));
    }

    #[test]
    fn write_and_partial_read() {
        let db = setup_db("write_and_partial_read", 2);

        let mut batch = db.batch().unwrap();
        batch.insert(0, &[0, 0], &[5, 4, 3, 2]).unwrap();
        batch.insert(1, &[1, 1], &[1, 2, 3, 4, 5]).unwrap();
        batch.commit().unwrap();

        assert_eq!(
            Some(vec![2, 3, 4]),
            db.partial_read(1, &[1, 1], &(1..4)).unwrap()
        );
        assert_eq!(None, db.partial_read(1, &[0, 0], &(1..4)).unwrap());
        // return None when invalid range is passed
        assert_eq!(None, db.partial_read(1, &[1, 1], &(2..8)).unwrap());
        // range must be increasing
        assert_eq!(None, db.partial_read(1, &[1, 1], &(3..0)).unwrap());

        assert_eq!(
            Some(vec![4, 3, 2]),
            db.partial_read(0, &[0, 0], &(1..4)).unwrap()
        );
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

    #[test]
    fn iter() {
        let db = setup_db("iter", 1);

        let mut batch = db.batch().unwrap();
        batch.insert(0, &[0, 0, 1], &[0, 0, 1]).unwrap();
        batch.insert(0, &[0, 1, 1], &[0, 1, 1]).unwrap();
        batch.insert(0, &[0, 1, 2], &[0, 1, 2]).unwrap();
        batch.insert(0, &[0, 1, 3], &[0, 1, 3]).unwrap();
        batch.insert(0, &[0, 2, 1], &[0, 2, 1]).unwrap();
        batch.commit().unwrap();

        let mut iter = db.iter(0, &[0, 1], Direction::Forward).unwrap();
        assert_eq!(
            (
                vec![0, 1, 1].into_boxed_slice(),
                vec![0, 1, 1].into_boxed_slice()
            ),
            iter.next().unwrap()
        );
        assert_eq!(
            (
                vec![0, 1, 2].into_boxed_slice(),
                vec![0, 1, 2].into_boxed_slice()
            ),
            iter.next().unwrap()
        );

        let mut iter = db.iter(0, &[0, 2], Direction::Reverse).unwrap();
        assert_eq!(
            (
                vec![0, 1, 3].into_boxed_slice(),
                vec![0, 1, 3].into_boxed_slice()
            ),
            iter.next().unwrap()
        );
        assert_eq!(
            (
                vec![0, 1, 2].into_boxed_slice(),
                vec![0, 1, 2].into_boxed_slice()
            ),
            iter.next().unwrap()
        );
    }
}
