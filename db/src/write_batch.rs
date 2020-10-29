//! TODO(doc): @quake
use crate::db::cf_handle;
use crate::{internal_error, Col, Result};
use rocksdb::{OptimisticTransactionDB, WriteBatch};
use std::sync::Arc;

/// TODO(doc): @quake
pub struct RocksDBWriteBatch {
    pub(crate) db: Arc<OptimisticTransactionDB>,
    pub(crate) inner: WriteBatch,
}

impl RocksDBWriteBatch {
    /// TODO(doc): @quake
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Return WriteBatch serialized size (in bytes).
    pub fn size_in_bytes(&self) -> usize {
        self.inner.size_in_bytes()
    }

    /// TODO(doc): @quake
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// TODO(doc): @quake
    pub fn put(&mut self, col: Col, key: &[u8], value: &[u8]) -> Result<()> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.put_cf(cf, key, value).map_err(internal_error)
    }

    /// TODO(doc): @quake
    pub fn delete(&mut self, col: Col, key: &[u8]) -> Result<()> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.delete_cf(cf, key).map_err(internal_error)
    }

    /// Remove database entries from start key to end key.
    ///
    /// Removes the database entries in the range ["begin_key", "end_key"), i.e.,
    /// including "begin_key" and excluding "end_key". It is not an error if no
    /// keys exist in the range ["begin_key", "end_key").
    pub fn delete_range(&mut self, col: Col, from: &[u8], to: &[u8]) -> Result<()> {
        let cf = cf_handle(&self.db, col)?;
        self.inner
            .delete_range_cf(cf, from, to)
            .map_err(internal_error)
    }

    /// TODO(doc): @quake
    pub fn clear(&mut self) -> Result<()> {
        self.inner.clear().map_err(internal_error)
    }
}
