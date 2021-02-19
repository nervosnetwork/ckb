//! RocksDB iterator wrapper base on DBIter
use crate::db::cf_handle;
use crate::{
    internal_error, Result, RocksDB, RocksDBSnapshot, RocksDBTransaction,
    RocksDBTransactionSnapshot,
};
use ckb_db_schema::Col;
use rocksdb::{ops::IterateCF, ReadOptions};
pub use rocksdb::{DBIterator as DBIter, Direction, IteratorMode};

/// An iterator over a column family, with specifiable ranges and direction.
pub trait DBIterator {
    /// Opens an interator using the provided IteratorMode.
    /// This is used when you want to iterate over a specific ColumnFamily
    fn iter(&self, col: Col, mode: IteratorMode) -> Result<DBIter> {
        let opts = ReadOptions::default();
        self.iter_opt(col, mode, &opts)
    }

    /// Opens an interator using the provided IteratorMode and ReadOptions.
    /// This is used when you want to iterate over a specific ColumnFamily with a modified ReadOptions
    fn iter_opt(&self, col: Col, mode: IteratorMode, readopts: &ReadOptions) -> Result<DBIter>;
}

impl DBIterator for RocksDB {
    fn iter_opt(&self, col: Col, mode: IteratorMode, readopts: &ReadOptions) -> Result<DBIter> {
        let cf = cf_handle(&self.inner, col)?;
        self.inner
            .iterator_cf_opt(cf, mode, readopts)
            .map_err(internal_error)
    }
}

impl DBIterator for RocksDBTransaction {
    fn iter_opt(&self, col: Col, mode: IteratorMode, readopts: &ReadOptions) -> Result<DBIter> {
        let cf = cf_handle(&self.db, col)?;
        self.inner
            .iterator_cf_opt(cf, mode, readopts)
            .map_err(internal_error)
    }
}

impl<'a> DBIterator for RocksDBTransactionSnapshot<'a> {
    fn iter_opt(&self, col: Col, mode: IteratorMode, readopts: &ReadOptions) -> Result<DBIter> {
        let cf = cf_handle(&self.db, col)?;
        self.inner
            .iterator_cf_opt(cf, mode, readopts)
            .map_err(internal_error)
    }
}

impl DBIterator for RocksDBSnapshot {
    fn iter_opt(&self, col: Col, mode: IteratorMode, readopts: &ReadOptions) -> Result<DBIter> {
        let cf = cf_handle(&self.db, col)?;
        self.iterator_cf_opt(cf, mode, readopts)
            .map_err(internal_error)
    }
}
