use crate::db::cf_handle;
use crate::{internal_error, Col, Result};
use rocksdb::ops::{DeleteCF, GetCF, Put, PutCF};
pub use rocksdb::{DBPinnableSlice, DBVector};
use rocksdb::{
    OptimisticTransaction, OptimisticTransactionDB, OptimisticTransactionSnapshot, ReadOptions,
};
use std::sync::Arc;

pub struct RocksDBTransaction {
    pub(crate) db: Arc<OptimisticTransactionDB>,
    pub(crate) inner: OptimisticTransaction,
}

impl RocksDBTransaction {
    pub fn get(&self, col: Col, key: &[u8]) -> Result<Option<DBVector>> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.get_cf(cf, key).map_err(internal_error)
    }

    pub fn put(&self, col: Col, key: &[u8], value: &[u8]) -> Result<()> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.put_cf(cf, key, value).map_err(internal_error)
    }

    pub fn put_default(&self, key: &[u8], value: &[u8]) -> Result<()> {
        self.inner.put(key, value).map_err(internal_error)
    }

    pub fn delete(&self, col: Col, key: &[u8]) -> Result<()> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.delete_cf(cf, key).map_err(internal_error)
    }

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

    pub fn commit(&self) -> Result<()> {
        self.inner.commit().map_err(internal_error)
    }

    pub fn rollback(&self) -> Result<()> {
        self.inner.rollback().map_err(internal_error)
    }

    pub fn get_snapshot(&self) -> RocksDBTransactionSnapshot<'_> {
        RocksDBTransactionSnapshot {
            db: Arc::clone(&self.db),
            inner: self.inner.snapshot(),
        }
    }

    pub fn set_savepoint(&self) {
        self.inner.set_savepoint()
    }

    pub fn rollback_to_savepoint(&self) -> Result<()> {
        self.inner.rollback_to_savepoint().map_err(internal_error)
    }
}

pub struct RocksDBTransactionSnapshot<'a> {
    pub(crate) db: Arc<OptimisticTransactionDB>,
    pub(crate) inner: OptimisticTransactionSnapshot<'a>,
}

impl<'a> RocksDBTransactionSnapshot<'a> {
    pub fn get(&self, col: Col, key: &[u8]) -> Result<Option<DBVector>> {
        let cf = cf_handle(&self.db, col)?;
        self.inner.get_cf(cf, key).map_err(internal_error)
    }
}
