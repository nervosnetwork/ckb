use crate::config::RocksDBConfig;
use crate::batch::{Batch, Col, Operation};
use crate::kvdb::{ErrorKind, KeyValueDB, Result};
use rocksdb::{ColumnFamily, WriteBatch, DB};
use std::ops::Range;

struct Inner {
    db: DB,
    cfnames: Vec<String>,
}

pub struct RocksDB {
    inner: Inner,
}

impl RocksDB {
    pub fn open(config: &RocksDBConfig, columns: u32) -> Self {
        let opts = config.to_db_options();
        let cfnames: Vec<_> = (0..columns).map(|c| format!("c{}", c)).collect();
        let cf_options: Vec<&str> = cfnames.iter().map(|n| n as &str).collect();
        let db = DB::open_cf(&opts, &config.path, &cf_options).expect("rocksdb open");
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
    fn cols(&self) -> u32 {
        self.inner.cfnames.len() as u32
    }

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

    fn len(&self, col: Col, key: &[u8]) -> Result<Option<usize>> {
        match self.cf_handle(col)? {
            Some(cf) => self.inner.db.get_pinned_cf(cf, &key),
            None => self.inner.db.get_pinned(&key),
        }
        .map(|v| v.map(|vi| vi.len()))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile;

    fn setup_db(prefix: &str, columns: u32) -> RocksDB {
        let tmp_dir = tempfile::Builder::new()
            .prefix(prefix).tempdir().unwrap();
        let config = RocksDBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };

        RocksDB::open(&config, columns)
    }

    #[test]
    #[should_panic]
    fn test_panic_if_missing() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_panic_if_missing").tempdir().unwrap();
        let config = RocksDBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            create_if_missing: Some(false),
            ..Default::default()
        };
        RocksDB::open(&config, 2); // panic
    }

    #[test]
    fn test_enable_statistics() {
        let opts = RocksDBConfig::default().to_db_options();
        assert!(opts.get_statistics().is_none());

        let tmp_dir = tempfile::Builder::new()
            .prefix("test_enable_statistics").tempdir().unwrap();
        let config = RocksDBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            enable_statistics: Some("".to_owned()),
            set_stats_dump_period_sec: Some(60),
            ..Default::default()
        };
        let opts = config.to_db_options();
        assert!(opts.get_statistics().is_some());
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
    fn write_and_len() {
        let db = setup_db("write_and_len", 2);

        let mut batch = Batch::default();
        batch.insert(None, vec![0, 0], vec![5, 4, 3, 2]);
        batch.insert(Some(1), vec![1, 1], vec![1, 2, 3, 4, 5]);
        db.write(batch).unwrap();

        assert_eq!(Some(4), db.len(None, &[0, 0]).unwrap());

        assert_eq!(Some(5), db.len(Some(1), &[1, 1]).unwrap());
        assert_eq!(None, db.len(Some(1), &[2, 2]).unwrap());
        assert!(db.len(Some(2), &[1, 1]).is_err());
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
