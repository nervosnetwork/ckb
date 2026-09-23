//! RocksDB snapshot wrapper
use crate::generation::Generation;
use crate::{DBPinnableSlice, Result};
use ckb_db_schema::Col;
use rocksdb::ops::{Iterate, IterateCF};
use rocksdb::{
    ColumnFamily, ConstHandle, DBRawIterator, Error, OptimisticTransactionDB, ReadOptions, ffi,
};
use std::sync::Arc;

/// A snapshot captures a point-in-time view of the DB at the time it's created
pub struct RocksDBSnapshot {
    pub(crate) db: Arc<OptimisticTransactionDB>,
    pub(crate) inner: *const ffi::rocksdb_snapshot_t,
    pub(crate) generation: Arc<Generation>,
}

unsafe impl Sync for RocksDBSnapshot {}
unsafe impl Send for RocksDBSnapshot {}

impl RocksDBSnapshot {
    /// # Safety
    ///
    /// `ptr` must be a live snapshot acquired from `db`, and `generation` must
    /// belong to that database. This wrapper takes responsibility for release.
    pub(crate) unsafe fn new(
        db: &Arc<OptimisticTransactionDB>,
        generation: Arc<Generation>,
        ptr: *const ffi::rocksdb_snapshot_t,
    ) -> RocksDBSnapshot {
        RocksDBSnapshot {
            db: Arc::clone(db),
            inner: ptr,
            generation,
        }
    }

    /// Return the value associated with a key using RocksDB's PinnableSlice from the given column
    /// so as to avoid unnecessary memory copy.
    pub fn get_pinned(&self, col: Col, key: &[u8]) -> Result<Option<DBPinnableSlice<'_>>> {
        let mut options = ReadOptions::default();
        // The generation belongs to this DB and the result borrows this snapshot.
        unsafe { options.set_snapshot(self) };
        self.generation.get_pinned(col, key, &options)
    }
}

// The owned DB outlives this snapshot; Drop releases the native handle once.
unsafe impl ConstHandle<ffi::rocksdb_snapshot_t> for RocksDBSnapshot {
    fn const_handle(&self) -> *const ffi::rocksdb_snapshot_t {
        self.inner
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
    fn get_raw_iter<'a: 'b, 'b>(&'a self, readopts: &ReadOptions) -> DBRawIterator<'b> {
        let mut ro = readopts.to_owned();
        // The returned iterator borrows this snapshot and uses its database.
        unsafe { ro.set_snapshot(self) };
        self.db.get_raw_iter(&ro)
    }
}

impl IterateCF for RocksDBSnapshot {
    fn get_raw_iter_cf<'a: 'b, 'b>(
        &'a self,
        cf_handle: &'b ColumnFamily,
        readopts: &ReadOptions,
    ) -> ::std::result::Result<DBRawIterator<'b>, Error> {
        let mut ro = readopts.to_owned();
        // The returned iterator borrows this snapshot and uses its database.
        unsafe { ro.set_snapshot(self) };
        self.db.get_raw_iter_cf(cf_handle, &ro)
    }
}
