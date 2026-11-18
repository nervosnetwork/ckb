use crate::cache::StoreCache;
use crate::store::ChainStore;
use ckb_db::{
    DBPinnableSlice, RocksDBSnapshot,
    iter::{DBIter, DBIterator, IteratorMode},
};
use ckb_db_schema::Col;
use ckb_freezer::Freezer;
use std::sync::Arc;

/// A snapshot of the chain store.
pub struct StoreSnapshot {
    pub(crate) inner: RocksDBSnapshot,
    pub(crate) freezer: Option<Freezer>,
    pub(crate) cache: Arc<StoreCache>,
}

impl ChainStore for StoreSnapshot {
    fn segment_value(
        &self,
        start: u64,
        key: &[u8],
    ) -> Result<Option<DBPinnableSlice<'_>>, ckb_error::Error> {
        self.inner.segment_get(start, key)
    }

    fn segment_rows(&self, start: u64, mode: IteratorMode) -> Result<DBIter<'_>, ckb_error::Error> {
        self.inner.segment_iter(start, mode)
    }

    fn cache(&self) -> Option<&StoreCache> {
        Some(&self.cache)
    }

    fn freezer(&self) -> Option<&Freezer> {
        self.freezer.as_ref()
    }

    fn get(&self, col: Col, key: &[u8]) -> Option<DBPinnableSlice<'_>> {
        self.inner
            .get_pinned(col, key)
            .expect("db operation should be ok")
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter<'_> {
        self.inner
            .iter(col, mode)
            .expect("db operation should be ok")
    }
}
