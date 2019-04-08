use crate::batch::{Batch, Col, Operation};
use crate::config::DBConfig;
use crate::kvdb::{ErrorKind, KeyValueDB, Result};
use log::warn;
use rocksdb::{ColumnFamily, Options, WriteBatch, DB};
use std::ops::Range;

struct Inner {
    db: DB,
    cfnames: Vec<String>,
}

pub struct RocksDB {
    inner: Inner,
}

impl RocksDB {
    pub fn open(config: &DBConfig, columns: u32) -> Self {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let cfnames: Vec<_> = (0..columns).map(|c| format!("c{}", c)).collect();
        let cf_options: Vec<&str> = cfnames.iter().map(|n| n as &str).collect();
        let db = DB::open_cf(&opts, &config.path, &cf_options).unwrap_or_else(|err| {
            if err.as_ref().starts_with("Corruption:") {
                warn!("Try repairing the rocksdb since {} ...", err);
                let mut repair_opts = Options::default();
                repair_opts.create_if_missing(false);
                repair_opts.create_missing_column_families(false);
                DB::repair(repair_opts, &config.path)
                    .unwrap_or_else(|err| panic!("Failed to repair the rocksdb: {}", err));
                warn!("Try opening the repaired rocksdb ...");
                DB::open_cf(&opts, &config.path, &cf_options)
                    .unwrap_or_else(|err| panic!("Failed to open the repaired rocksdb: {}", err))
            } else {
                panic!("Failed to open rocksdb: {}", err);
            }
        });

        if let Some(db_opt) = config.options.as_ref() {
            let rocksdb_options: Vec<(&str, &str)> = db_opt
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            db.set_options(&rocksdb_options)
                .expect("Failed to set rocksdb option");
        }

        let inner = Inner {
            db,
            cfnames: cfnames.clone(),
        };
        RocksDB { inner }
    }

    fn cf_handle(&self, col: Option<u32>) -> Result<Option<ColumnFamily>> {
        if let Some(col) = col {
            self.inner
                .cfnames
                .get(col as usize)
                .ok_or_else(|| ErrorKind::DBError(format!("column {:?} not found ", col)))
                .map(|cfname| self.inner.db.cf_handle(&cfname))
        } else {
            Ok(None)
        }
    }
}

impl KeyValueDB for RocksDB {
    fn write(&self, batch: Batch) -> Result<()> {
        let mut wb = WriteBatch::default();
        for op in batch.operations {
            match op {
                Operation::Insert { col, key, value } => match self.cf_handle(col)? {
                    Some(cf) => wb.put_cf(cf, &key, &value),
                    None => wb.put(&key, &value),
                },
                Operation::Delete { col, key } => match self.cf_handle(col)? {
                    None => wb.delete(&key),
                    Some(cf) => wb.delete_cf(cf, &key),
                },
            }?;
        }
        self.inner.db.write(wb)?;
        Ok(())
    }

    fn read(&self, col: Col, key: &[u8]) -> Result<Option<Vec<u8>>> {
        match self.cf_handle(col)? {
            Some(cf) => self.inner.db.get_cf(cf, &key),
            None => self.inner.db.get(&key),
        }
        .map(|v| v.map(|vi| vi.to_vec()))
        .map_err(Into::into)
    }

    fn partial_read(&self, col: Col, key: &[u8], range: &Range<usize>) -> Result<Option<Vec<u8>>> {
        match self.cf_handle(col)? {
            Some(cf) => self.inner.db.get_pinned_cf(cf, &key),
            None => self.inner.db.get_pinned(&key),
        }
        .map(|v| v.and_then(|vi| vi.get(range.start..range.end).map(|slice| slice.to_vec())))
        .map_err(Into::into)
    }

    fn iter(&self, col: Col, key: &[u8]) -> Option<DBIterator> {
        self.cf_handle(col).expect("invalid col").map(|cf| {
            self.inner
                .db
                .iterator_cf(cf, IteratorMode::From(key, Direction::Forward))
                .expect("invalid iterator")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile;

    fn setup_db(prefix: &str, columns: u32) -> RocksDB {
        let tmp_dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };

        RocksDB::open(&config, columns)
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

        let mut batch = Batch::default();
        batch.insert(None, vec![0, 0], vec![0, 0, 0]);
        batch.insert(Some(1), vec![1, 1], vec![1, 1, 1]);
        db.write(batch).unwrap();

        assert_eq!(Some(vec![0, 0, 0]), db.read(None, &[0, 0]).unwrap());
        assert_eq!(None, db.read(None, &[1, 1]).unwrap());

        assert_eq!(None, db.read(Some(1), &[0, 0]).unwrap());
        assert_eq!(Some(vec![1, 1, 1]), db.read(Some(1), &[1, 1]).unwrap());

        // return err when col doesn't exist
        assert!(db.read(Some(2), &[0, 0]).is_err());
    }

    #[test]
    fn write_and_partial_read() {
        let db = setup_db("write_and_partial_read", 2);

        let mut batch = Batch::default();
        batch.insert(None, vec![0, 0], vec![5, 4, 3, 2]);
        batch.insert(Some(1), vec![1, 1], vec![1, 2, 3, 4, 5]);
        db.write(batch).unwrap();

        assert_eq!(
            Some(vec![2, 3, 4]),
            db.partial_read(Some(1), &[1, 1], &(1..4)).unwrap()
        );
        assert_eq!(None, db.partial_read(Some(1), &[0, 0], &(1..4)).unwrap());
        // return None when invalid range is passed
        assert_eq!(None, db.partial_read(Some(1), &[1, 1], &(2..8)).unwrap());
        // range must be increasing
        assert_eq!(None, db.partial_read(Some(1), &[1, 1], &(3..0)).unwrap());
        // return err when col doesn't exist
        assert!(db.partial_read(Some(2), &[0, 0], &(0..1)).is_err());

        assert_eq!(
            Some(vec![4, 3, 2]),
            db.partial_read(None, &[0, 0], &(1..4)).unwrap()
        );
    }
}
