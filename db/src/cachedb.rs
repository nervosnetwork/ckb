use crate::{Col, DbBatch, KeyValueDB, Result};
use ckb_util::Mutex;
use fnv::FnvHashMap;
use lru_cache::LruCache;
use std::ops::Range;
use std::sync::Arc;

type CacheTable = FnvHashMap<Col, Mutex<LruCache<Vec<u8>, Vec<u8>>>>;
pub type CacheCols = (u32, usize);

enum BatchOperation {
    Insert {
        col: Col,
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Delete {
        col: Col,
        key: Vec<u8>,
    },
}

pub struct CacheDBBatch<T>
where
    T: DbBatch,
{
    inner: T,
    cache: Arc<CacheTable>,
    operations: Vec<BatchOperation>,
}

impl<T: DbBatch> CacheDBBatch<T> {
    fn new(inner: T, cache: Arc<CacheTable>) -> CacheDBBatch<T> {
        Self {
            inner,
            cache,
            operations: Vec::new(),
        }
    }
}

impl<T: DbBatch> DbBatch for CacheDBBatch<T> {
    fn insert(&mut self, col: Col, key: &[u8], value: &[u8]) -> Result<()> {
        self.inner.insert(col, key, value)?;
        if self.cache.contains_key(&col) {
            self.operations.push(BatchOperation::Insert {
                col,
                key: key.to_vec(),
                value: value.to_vec(),
            });
        }
        Ok(())
    }

    fn delete(&mut self, col: Col, key: &[u8]) -> Result<()> {
        self.inner.delete(col, key)?;
        if self.cache.contains_key(&col) {
            self.operations.push(BatchOperation::Delete {
                col,
                key: key.to_vec(),
            });
        }
        Ok(())
    }

    fn commit(self) -> Result<()> {
        self.inner.commit()?;
        for op in self.operations {
            match op {
                BatchOperation::Insert { col, key, value } => {
                    if let Some(cache) = self.cache.get(&col) {
                        let mut cache_guard = cache.lock();
                        cache_guard.insert(key, value);
                    }
                }
                BatchOperation::Delete { col, key } => {
                    if let Some(cache) = self.cache.get(&col) {
                        let mut cache_guard = cache.lock();
                        cache_guard.remove(&key);
                    }
                }
            }
        }
        Ok(())
    }
}

pub struct CacheDB<T>
where
    T: KeyValueDB,
{
    db: T,
    cache: Arc<CacheTable>,
}

impl<T> CacheDB<T>
where
    T: KeyValueDB,
{
    pub fn new(db: T, cols: &[CacheCols]) -> Self {
        let mut table = FnvHashMap::with_capacity_and_hasher(cols.len(), Default::default());
        for (idx, capacity) in cols {
            table.insert(*idx, Mutex::new(LruCache::new(*capacity)));
        }
        CacheDB {
            db,
            cache: Arc::new(table),
        }
    }
}

impl<T> KeyValueDB for CacheDB<T>
where
    T: KeyValueDB,
{
    type Batch = CacheDBBatch<T::Batch>;

    fn read(&self, col: Col, key: &[u8]) -> Result<Option<Vec<u8>>> {
        if let Some(value) = self
            .cache
            .get(&col)
            .and_then(|cache| cache.lock().get_refresh(key).cloned())
        {
            return Ok(Some(value));
        }
        self.db.read(col, key)
    }

    fn partial_read(&self, col: Col, key: &[u8], range: &Range<usize>) -> Result<Option<Vec<u8>>> {
        if let Some(cache) = self.cache.get(&col) {
            let mut cache_guard = cache.lock();
            if let Some(data) = cache_guard.get_refresh(key) {
                return Ok(data.get(range.start..range.end).map(|slice| slice.to_vec()));
            }
        }
        self.db.partial_read(col, key, range)
    }

    fn traverse<F>(&self, col: Col, callback: F) -> Result<()>
    where
        F: FnMut(&[u8], &[u8]) -> Result<()>,
    {
        self.db.traverse(col, callback)
    }

    fn batch(&self) -> Result<Self::Batch> {
        Ok(CacheDBBatch::new(self.db.batch()?, Arc::clone(&self.cache)))
    }
}
