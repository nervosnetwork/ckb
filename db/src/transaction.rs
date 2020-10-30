//! TODO(doc): @quake
use crate::db::cf_handle;
use crate::{internal_error, Col, Result};
use rocksdb::ops::{DeleteCF, GetCF, PutCF};
pub use rocksdb::{DBPinnableSlice, DBVector};
use rocksdb::{
    OptimisticTransaction, OptimisticTransactionDB, OptimisticTransactionSnapshot, ReadOptions,
};
use std::sync::Arc;

/// TODO(doc): @quake
pub struct RocksDBTransaction {
    pub(crate) db: Arc<OptimisticTransactionDB>,
    pub(crate) inner: OptimisticTransaction,
}

impl RocksDBTransaction {
    /// TODO(doc): @quake
    pub fn get(&self, col: Col, key: &[u8]) -> Result<Option<DBVector>> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.get_cf(cf, key).map_err(internal_error)
    }

    /// TODO(doc): @quake
    pub fn put(&self, col: Col, key: &[u8], value: &[u8]) -> Result<()> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.put_cf(cf, key, value).map_err(internal_error)
    }

    /// TODO(doc): @quake
    pub fn delete(&self, col: Col, key: &[u8]) -> Result<()> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.delete_cf(cf, key).map_err(internal_error)
    }

    /// TODO(doc): @quake
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

    /// TODO(doc): @quake
    pub fn commit(&self) -> Result<()> {
        self.inner.commit().map_err(internal_error)
    }

    /// TODO(doc): @quake
    pub fn rollback(&self) -> Result<()> {
        self.inner.rollback().map_err(internal_error)
    }

    /// TODO(doc): @quake
    pub fn get_snapshot(&self) -> RocksDBTransactionSnapshot<'_> {
        RocksDBTransactionSnapshot {
            db: Arc::clone(&self.db),
            inner: self.inner.snapshot(),
        }
    }

    /// TODO(doc): @quake
    pub fn set_savepoint(&self) {
        self.inner.set_savepoint()
    }

    /// TODO(doc): @quake
    pub fn rollback_to_savepoint(&self) -> Result<()> {
        self.inner.rollback_to_savepoint().map_err(internal_error)
    }
}

/// TODO(doc): @quake
pub struct RocksDBTransactionSnapshot<'a> {
    pub(crate) db: Arc<OptimisticTransactionDB>,
    pub(crate) inner: OptimisticTransactionSnapshot<'a>,
}

impl<'a> RocksDBTransactionSnapshot<'a> {
    /// TODO(doc): @quake
    pub fn get(&self, col: Col, key: &[u8]) -> Result<Option<DBVector>> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.get_cf(cf, key).map_err(internal_error)
    }
}
