use ckb_db::{Col, KeyValueDB, Result};
use ckb_util::RwLock;
use fnv::FnvHashMap;
use lru_cache::LruCache;
use std::ops::Range;

type CacheTable = FnvHashMap<Col, LruCache<Vec<u8>, Vec<u8>>>;
pub type CacheCols = (u32, usize);

pub struct CacheDB<T>
where
    T: KeyValueDB,
{
    db: T,
    cache: RwLock<CacheTable>,
}

impl<T> CacheDB<T>
where
    T: KeyValueDB,
{
    pub fn new(db: T, cols: &[CacheCols]) -> Self {
        let mut table = FnvHashMap::with_capacity_and_hasher(cols.len(), Default::default());
        for (idx, capacity) in cols {
            table.insert(*idx, LruCache::new(*capacity));
        }
        CacheDB {
            db,
            cache: RwLock::new(table),
        }
    }
}

impl<T> KeyValueDB for CacheDB<T>
where
    T: KeyValueDB,
{
    type Batch = T::Batch;

    fn read(&self, col: Col, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let cache_guard = self.cache.read();
        if let Some(value) = cache_guard
            .get(&col)
            .and_then(|cache| cache.get(key))
            .cloned()
        {
            return Ok(Some(value));
        }
        self.db.read(col, key)
    }

    fn partial_read(&self, col: Col, key: &[u8], range: &Range<usize>) -> Result<Option<Vec<u8>>> {
        let cache_guard = self.cache.read();
        if let Some(data) = cache_guard.get(&col).and_then(|cache| cache.get(key)) {
            return Ok(data.get(range.start..range.end).map(|slice| slice.to_vec()));
        }
        self.db.partial_read(col, key, range)
    }

    fn batch(&self) -> Result<Self::Batch> {
        self.db.batch()
    }
}
