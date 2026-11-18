//! RocksDB write batch wrapper
use crate::generation::{Generation, WriterPermit};
use crate::{Result, internal_error};
use ckb_db_schema::Col;
use rocksdb::{OptimisticTransactionDB, WriteBatch};
use std::sync::Arc;

/// An atomic batch of write operations.
///
/// Making an atomic commit of several write operations.
#[derive(Clone)]
pub struct RocksDBWriteBatch {
    pub(crate) inner: WriteBatch,
    pub(crate) generation: Arc<Generation>,
    pub(crate) db: Arc<OptimisticTransactionDB>,
    pub(crate) permit: Arc<WriterPermit>,
}

impl RocksDBWriteBatch {
    /// Return the count of write batch.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Return WriteBatch serialized size (in bytes).
    pub fn size_in_bytes(&self) -> usize {
        self.inner.size_in_bytes()
    }

    /// Returns true if the write batch contains no operations.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Write the bytes into the given column with associated key.
    pub fn put(&mut self, col: Col, key: &[u8], value: &[u8]) -> Result<()> {
        let cf = self.generation.cf(col)?;
        self.permit.record(col, key)?;
        self.inner.put_cf(cf, key, value).map_err(internal_error)
    }

    /// Delete the data associated with the given key and given column.
    pub fn delete(&mut self, col: Col, key: &[u8]) -> Result<()> {
        let cf = self.generation.cf(col)?;
        self.permit.record(col, key)?;
        self.inner.delete_cf(cf, key).map_err(internal_error)
    }

    /// Delete each key yielded by the iterator. Missing keys are ignored.
    pub fn delete_range<K: AsRef<[u8]>>(
        &mut self,
        col: Col,
        range: impl Iterator<Item = K>,
    ) -> Result<()> {
        let cf = self.generation.cf(col)?;

        // OptimisticTransactionDB does not support range tombstones.
        for key in range {
            let key = key.as_ref();
            self.permit.record(col, key)?;
            self.inner.delete_cf(cf, key).map_err(internal_error)?;
        }
        Ok(())
    }

    /// Clear all updates buffered in this batch.
    pub fn clear(&mut self) -> Result<()> {
        self.inner.clear().map_err(internal_error)
    }
}
