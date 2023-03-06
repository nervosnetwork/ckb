//! ReadOnlyDB wrapper base on rocksdb read_only_open mode
use crate::{internal_error, Result};
use ckb_db_schema::Col;
use ckb_logger::info;
use rocksdb::ops::{GetColumnFamilys, GetPinned, GetPinnedCF, OpenCF};
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
    pub fn open_cf<P, I, N>(path: P, cf_names: I) -> Result<Option<Self>>
    where
        P: AsRef<Path>,
        I: IntoIterator<Item = N>,
        N: AsRef<str>,
    {
        let opts = Options::default();
        RawReadOnlyDB::open_cf(&opts, path, cf_names).map_or_else(
            |err| {
                let err_str = err.as_ref();
                // notice: err msg difference
                if err_str.starts_with("IO error: No such file or directory")
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
                        "failed to open the database: {err}"
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
        self.inner.get_pinned(key).map_err(internal_error)
    }

    /// Return the value associated with a key using RocksDB's PinnableSlice from the given column
    /// so as to avoid unnecessary memory copy.
    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice>> {
        let cf = self
            .inner
            .cf_handle(col)
            .ok_or_else(|| internal_error(format!("column {col} not found")))?;
        self.inner.get_pinned_cf(cf, key).map_err(internal_error)
    }
}
