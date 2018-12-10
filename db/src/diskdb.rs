use batch::{Batch, Col, Operation};
use kvdb::{ErrorKind, KeyValueDB, Result};
use rocksdb::{ColumnFamily, Options, WriteBatch, DB};
use std::ops::Range;
use std::path::Path;

struct Inner {
    db: DB,
    cfnames: Vec<String>,
}

pub struct RocksDB {
    inner: Inner,
}

impl RocksDB {
    pub fn open<P: AsRef<Path>>(path: P, columns: u32) -> Self {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        let cfnames: Vec<_> = (0..columns).map(|c| format!("c{}", c)).collect();
        let cf_options: Vec<&str> = cfnames.iter().map(|n| n as &str).collect();
        let db = DB::open_cf(&opts, path, &cf_options).expect("rocksdb open");
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

    #[test]
    fn write_and_read() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("write_and_read")
            .tempdir()
            .unwrap();
        let db = RocksDB::open(tmp_dir, 2);
        let mut batch = Batch::default();
        batch.insert(None, vec![0, 0], vec![0, 0, 0]);
        batch.insert(Some(1), vec![1, 1], vec![1, 1, 1]);
        db.write(batch).unwrap();

        assert_eq!(Some(vec![0, 0, 0]), db.read(None, &vec![0, 0]).unwrap());
        assert_eq!(None, db.read(None, &vec![1, 1]).unwrap());

        assert_eq!(None, db.read(Some(1), &vec![0, 0]).unwrap());
        assert_eq!(Some(vec![1, 1, 1]), db.read(Some(1), &vec![1, 1]).unwrap());

        //return err when col doesn't exist
        assert!(db.read(Some(2), &vec![0, 0]).is_err());
    }

    #[test]
    fn write_and_len() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("write_and_len")
            .tempdir()
            .unwrap();
        let db = RocksDB::open(tmp_dir, 2);
        let mut batch = Batch::default();
        batch.insert(None, vec![0, 0], vec![5, 4, 3, 2]);
        batch.insert(Some(1), vec![1, 1], vec![1, 2, 3, 4, 5]);
        db.write(batch).unwrap();

        assert_eq!(Some(4), db.len(None, &vec![0, 0]).unwrap());

        assert_eq!(Some(5), db.len(Some(1), &vec![1, 1]).unwrap());
        assert_eq!(None, db.len(Some(1), &vec![2, 2]).unwrap());
        assert!(db.len(Some(2), &vec![1, 1]).is_err());
    }

    #[test]
    fn write_and_partial_read() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("write_and_partial_read")
            .tempdir()
            .unwrap();
        let db = RocksDB::open(tmp_dir, 2);
        let mut batch = Batch::default();
        batch.insert(None, vec![0, 0], vec![5, 4, 3, 2]);
        batch.insert(Some(1), vec![1, 1], vec![1, 2, 3, 4, 5]);
        db.write(batch).unwrap();

        assert_eq!(
            Some(vec![2, 3, 4]),
            db.partial_read(Some(1), &vec![1, 1], &(1..4)).unwrap()
        );
        assert_eq!(
            None,
            db.partial_read(Some(1), &vec![0, 0], &(1..4)).unwrap()
        );
        // return None when invalid range is passed
        assert_eq!(
            None,
            db.partial_read(Some(1), &vec![1, 1], &(2..8)).unwrap()
        );
        // range must be increasing
        assert_eq!(
            None,
            db.partial_read(Some(1), &vec![1, 1], &(3..0)).unwrap()
        );
        //return err when col doesn't exist
        assert!(db.partial_read(Some(2), &vec![0, 0], &(0..1)).is_err());

        assert_eq!(
            Some(vec![4, 3, 2]),
            db.partial_read(None, &vec![0, 0], &(1..4)).unwrap()
        );
    }
}
