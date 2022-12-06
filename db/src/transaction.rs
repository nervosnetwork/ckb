//! RocksDB optimistic transaction wrapper
use crate::db::cf_handle;
use crate::{internal_error, Result};
use ckb_db_schema::Col;
use rocksdb::ops::{DeleteCF, GetPinnedCF, PutCF};
pub use rocksdb::{DBPinnableSlice, DBVector};
use rocksdb::{
    OptimisticTransaction, OptimisticTransactionDB, OptimisticTransactionSnapshot, ReadOptions,
};
use std::sync::Arc;

/// An optimistic transaction database.
pub struct RocksDBTransaction {
    pub(crate) db: Arc<OptimisticTransactionDB>,
    pub(crate) inner: OptimisticTransaction,
}

impl RocksDBTransaction {
    /// Return the bytes associated with the given key and given column.
    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice>> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.get_pinned_cf(cf, key).map_err(internal_error)
    }

    /// Write the bytes into the given column with associated key.
    pub fn put(&self, col: Col, key: &[u8], value: &[u8]) -> Result<()> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.put_cf(cf, key, value).map_err(internal_error)
    }

    /// Delete the data associated with the given key and given column.
    pub fn delete(&self, col: Col, key: &[u8]) -> Result<()> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.delete_cf(cf, key).map_err(internal_error)
    }

    /// Read a key and make the read value a precondition for transaction commit.
    pub fn get_for_update<'a>(
        &self,
        col: Col,
        key: &[u8],
        snapshot: &RocksDBTransactionSnapshot<'a>,
    ) -> Result<Option<DBVector>> {
        let cf = cf_handle(&self.db, col)?;
        let mut opts = ReadOptions::default();
        opts.set_snapshot(&snapshot.inner);
        self.inner
            .get_for_update_cf_opt(cf, key, &opts, true)
            .map_err(internal_error)
    }

    /// Commit the transaction.
    pub fn commit(&self) -> Result<()> {
        self.inner.commit().map_err(internal_error)
    }

    /// Rollback the transaction.
    pub fn rollback(&self) -> Result<()> {
        self.inner.rollback().map_err(internal_error)
    }

    /// Return `RocksDBTransactionSnapshot`
    pub fn get_snapshot(&self) -> RocksDBTransactionSnapshot<'_> {
        RocksDBTransactionSnapshot {
            db: Arc::clone(&self.db),
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
    pub(crate) db: Arc<OptimisticTransactionDB>,
    pub(crate) inner: OptimisticTransactionSnapshot<'a>,
}

impl<'a> RocksDBTransactionSnapshot<'a> {
    /// Return the bytes associated with the given key and given column.
    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice>> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.get_pinned_cf(cf, key).map_err(internal_error)
    }
}
