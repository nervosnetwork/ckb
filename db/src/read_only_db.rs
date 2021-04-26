//! ReadOnlyDB wrapper base on rocksdb read_only_open mode
use crate::{internal_error, Result};
use ckb_logger::info;
use rocksdb::ops::{GetPinned, Open};
use rocksdb::{DBPinnableSlice, Options, ReadOnlyDB as RawReadOnlyDB};
use std::path::Path;
use std::sync::Arc;

/// ReadOnlyDB wrapper
pub struct ReadOnlyDB {
    pub(crate) inner: Arc<RawReadOnlyDB>,
}

impl ReadOnlyDB {
    /// The behavior is similar to DB::Open,
    /// except that it opens DB in read-only mode.
    /// One big difference is that when opening the DB as read-only,
    /// you don't need to specify all Column Families
    /// -- you can only open a subset of Column Families.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Option<Self>> {
        let opts = Options::default();
        RawReadOnlyDB::open(&opts, path).map_or_else(
            |err| {
                let err_str = err.as_ref();
                // notice: err msg difference
                if err_str.starts_with("NotFound")
                    && err_str.ends_with("does not exist")
                {
                    Ok(None)
                } else if err_str.starts_with("Corruption:") {
                    info!(
                        "DB corrupted: {}.\n\
                        Try ckb db-repair command to repair DB.\n\
                        Note: Currently there is a limitation that un-flushed column families will be lost after repair.\
                        This would happen even if the DB is in healthy state.\n\
                        See https://github.com/facebook/rocksdb/wiki/RocksDB-Repairer for detail",
                        err_str
                    );
                    Err(internal_error("DB corrupted"))
                } else {
                    Err(internal_error(format!(
                        "failed to open the database: {}",
                        err
                    )))
                }
            },
            |db| {
                Ok(Some(ReadOnlyDB {
                    inner: Arc::new(db),
                }))
            },
        )
    }

    /// Return the value associated with a key using RocksDB's PinnableSlice from the default column
    /// so as to avoid unnecessary memory copy.
    pub fn get_pinned_default(&self, key: &[u8]) -> Result<Option<DBPinnableSlice>> {
        self.inner.get_pinned(&key).map_err(internal_error)
    }
}

#[cfg(test)]
mod tests {
    use super::ReadOnlyDB;

    #[test]
    fn test_open_read_only_not_exist() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_open_read_only_not_exist")
            .tempdir()
            .unwrap();

        let db = ReadOnlyDB::open(&tmp_dir);
        assert!(matches!(db, Ok(x) if x.is_none()))
    }
}
