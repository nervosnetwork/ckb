use crate::db::cf_handle;
use crate::{Col, Result};
use rocksdb::ops::{GetCF, GetPinnedCF, Iterate, IterateCF, Read};
use rocksdb::{
    ffi, ColumnFamily, ConstHandle, DBPinnableSlice, DBRawIterator, DBVector, Error, Handle,
    OptimisticTransactionDB, ReadOptions,
};
use std::sync::Arc;

pub struct RocksDBSnapshot {
    pub(crate) db: Arc<OptimisticTransactionDB>,
    pub(crate) inner: *const ffi::rocksdb_snapshot_t,
    ro: ReadOptions,
}

unsafe impl Sync for RocksDBSnapshot {}
unsafe impl Send for RocksDBSnapshot {}

impl RocksDBSnapshot {
    pub unsafe fn new(
        db: &Arc<OptimisticTransactionDB>,
        ptr: *const ffi::rocksdb_snapshot_t,
    ) -> RocksDBSnapshot {
        let ro = ReadOptions::default();
        ffi::rocksdb_readoptions_set_snapshot(ro.handle(), ptr);
        RocksDBSnapshot {
            db: Arc::clone(db),
            ro,
            inner: ptr,
        }
    }

    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice>> {
        let cf = cf_handle(&self.db, col)?;
        self.db
            .get_pinned_cf_opt(cf, &key, &self.ro)
            .map_err(Into::into)
    }
}

impl Read for RocksDBSnapshot {}

impl ConstHandle<ffi::rocksdb_snapshot_t> for RocksDBSnapshot {
    fn const_handle(&self) -> *const ffi::rocksdb_snapshot_t {
        self.inner
    }
}

impl GetCF<ReadOptions> for RocksDBSnapshot {
    fn get_cf_full<K: AsRef<[u8]>>(
        &self,
        cf: Option<&ColumnFamily>,
        key: K,
        readopts: Option<&ReadOptions>,
    ) -> ::std::result::Result<Option<DBVector>, Error> {
        let mut ro = readopts.cloned().unwrap_or_default();
        ro.set_snapshot(self);

        self.db.get_cf_full(cf, key, Some(&ro))
    }
}

impl Drop for RocksDBSnapshot {
    fn drop(&mut self) {
        unsafe {
            ffi::rocksdb_release_snapshot(self.db.base_db_ptr(), self.inner);
        }
    }
}

impl Iterate for RocksDBSnapshot {
    fn get_raw_iter(&self, readopts: &ReadOptions) -> DBRawIterator {
        let mut ro = readopts.to_owned();
        ro.set_snapshot(self);
        self.db.get_raw_iter(&ro)
    }
}

impl IterateCF for RocksDBSnapshot {
    fn get_raw_iter_cf(
        &self,
        cf_handle: &ColumnFamily,
        readopts: &ReadOptions,
    ) -> ::std::result::Result<DBRawIterator, Error> {
        let mut ro = readopts.to_owned();
        ro.set_snapshot(self);
        self.db.get_raw_iter_cf(cf_handle, &ro)
    }
}
