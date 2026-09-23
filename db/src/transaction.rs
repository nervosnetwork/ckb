//! RocksDB optimistic transaction wrapper
use crate::generation::{Generation, WriterPermit};
use crate::{DBPinnableSlice, Result, internal_error};
use ckb_db_schema::Col;
pub use rocksdb::DBVector;
use rocksdb::ops::{DeleteCF, GetPinnedCF, PutCF};
use rocksdb::{OptimisticTransaction, OptimisticTransactionSnapshot, ReadOptions};
use std::sync::Arc;

/// An optimistic transaction database.
pub struct RocksDBTransaction {
    pub(crate) inner: OptimisticTransaction,
    pub(crate) generation: Arc<Generation>,
    pub(crate) permit: WriterPermit,
}

impl RocksDBTransaction {
    /// Return the bytes associated with the given key and given column.
    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice<'_>>> {
        let cf = self.generation.cf(col)?;
        self.inner.get_pinned_cf(cf, key).map_err(internal_error)
    }

    /// Write the bytes into the given column with associated key.
    pub fn put(&self, col: Col, key: &[u8], value: &[u8]) -> Result<()> {
        self.permit.record(col, key)?;
        let cf = self.generation.cf(col)?;
        self.inner.put_cf(cf, key, value).map_err(internal_error)
    }

    /// Delete the data associated with the given key and given column.
    pub fn delete(&self, col: Col, key: &[u8]) -> Result<()> {
        self.permit.record(col, key)?;
        let cf = self.generation.cf(col)?;
        self.inner.delete_cf(cf, key).map_err(internal_error)
    }

    /// Read a key and make the read value a precondition for transaction commit.
    pub fn get_for_update(
        &self,
        col: Col,
        key: &[u8],
        snapshot: &RocksDBTransactionSnapshot<'_>,
    ) -> Result<Option<DBVector>> {
        if !Arc::ptr_eq(&self.generation, &snapshot.generation) {
            return Err(internal_error(
                "transaction snapshot belongs to another generation",
            ));
        }
        let cf = self.generation.cf(col)?;
        let mut opts = ReadOptions::default();
        // The matching generation identifies the same DB. The borrowed snapshot
        // remains alive for this read, including when it belongs to another txn.
        unsafe { opts.set_snapshot(&snapshot.inner) };
        self.inner
            .get_for_update_cf_opt(cf, key, &opts, true)
            .map_err(internal_error)
    }

    /// Commit the transaction.
    pub fn commit(&self) -> Result<()> {
        self.permit.check()?;
        self.inner.commit().map_err(internal_error)
    }

    /// Rollback the transaction.
    pub fn rollback(&self) -> Result<()> {
        self.inner.rollback().map_err(internal_error)
    }

    /// Return `RocksDBTransactionSnapshot`
    pub fn get_snapshot(&self) -> RocksDBTransactionSnapshot<'_> {
        RocksDBTransactionSnapshot {
            generation: Arc::clone(&self.generation),
            inner: self.inner.snapshot(),
        }
    }

    /// Set savepoint for transaction.
    pub fn set_savepoint(&self) {
        self.inner.set_savepoint()
    }

    /// Rollback the transaction to savepoint.
    pub fn rollback_to_savepoint(&self) -> Result<()> {
        self.inner.rollback_to_savepoint().map_err(internal_error)
    }
}

/// A snapshot captures a point-in-time view of the transaction at the time it's created
pub struct RocksDBTransactionSnapshot<'a> {
    pub(crate) inner: OptimisticTransactionSnapshot<'a>,
    pub(crate) generation: Arc<Generation>,
}

impl<'a> RocksDBTransactionSnapshot<'a> {
    /// Return the bytes associated with the given key and given column.
    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice<'_>>> {
        let cf = self.generation.cf(col)?;
        self.inner.get_pinned_cf(cf, key).map_err(internal_error)
    }
}
