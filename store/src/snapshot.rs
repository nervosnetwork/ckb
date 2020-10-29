use crate::cache::StoreCache;
use crate::store::ChainStore;
use ckb_db::{
    iter::{DBIter, DBIterator, IteratorMode},
    Col, DBPinnableSlice, RocksDBSnapshot,
};
use std::sync::Arc;

/// TODO(doc): @quake
pub struct StoreSnapshot {
    pub(crate) inner: RocksDBSnapshot,
    pub(crate) cache: Arc<StoreCache>,
}

impl<'a> ChainStore<'a> for StoreSnapshot {
    type Vector = DBPinnableSlice<'a>;

    fn cache(&'a self) -> Option<&'a StoreCache> {
        Some(&self.cache)
    }

    fn get(&'a self, col: Col, key: &[u8]) -> Option<Self::Vector> {
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
