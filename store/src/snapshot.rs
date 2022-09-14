use crate::cache::StoreCache;
use crate::store::ChainStore;
use ckb_db::{
    iter::{DBIter, DBIterator, IteratorMode},
    DBPinnableSlice, RocksDBSnapshot,
};
use ckb_db_schema::Col;
use ckb_freezer::Freezer;
use std::sync::Arc;

/// TODO(doc): @quake
pub struct StoreSnapshot {
    pub(crate) inner: RocksDBSnapshot,
    pub(crate) freezer: Option<Freezer>,
    pub(crate) cache: Arc<StoreCache>,
}

impl ChainStore for StoreSnapshot {
    fn cache(&self) -> Option<&StoreCache> {
        Some(&self.cache)
    }

    fn freezer(&self) -> Option<&Freezer> {
        self.freezer.as_ref()
    }

    fn get(&self, col: Col, key: &[u8]) -> Option<DBPinnableSlice> {
        self.inner
            .get_pinned(col, key)
            .expect("db operation should be ok")
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter {
        self.inner
            .iter(col, mode)
            .expect("db operation should be ok")
    }
}
