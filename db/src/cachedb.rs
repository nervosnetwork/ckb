use batch::{Batch, Key, KeyValue, Operation, Value};
use bigint::H256;
use core::block::Header;
use kvdb::{KeyValueDB, Result};
use lru_cache::LruCache;
use util::Mutex;

pub struct CacheKeyValueDB<T>
where
    T: KeyValueDB,
{
    db: T,
    block_header: Mutex<LruCache<H256, Header>>,
    block_hash: Mutex<LruCache<u64, H256>>,
}

impl<T> CacheKeyValueDB<T>
where
    T: KeyValueDB,
{
    pub fn new(db: T) -> Self {
        CacheKeyValueDB {
            db,
            block_header: Mutex::new(LruCache::new(4096)),
            block_hash: Mutex::new(LruCache::new(4096)),
        }
    }
}

impl<T> KeyValueDB for CacheKeyValueDB<T>
where
    T: KeyValueDB,
{
    fn write(&self, batch: Batch) -> Result<()> {
        for op in &batch.operations {
            match *op {
                Operation::Insert(KeyValue::BlockHeader(hash, ref header)) => {
                    self.block_header.lock().insert(hash, *header.clone());
                }
                Operation::Insert(KeyValue::BlockHash(height, hash)) => {
                    self.block_hash.lock().insert(height, hash);
                }
                Operation::Delete(Key::BlockHeader(hash)) => {
                    self.block_header.lock().remove(&hash);
                }
                Operation::Delete(Key::BlockHash(height)) => {
                    self.block_hash.lock().remove(&height);
                }
                _ => (),
            }
        }
        self.db.write(batch)
    }

    fn read(&self, key: &Key) -> Result<Option<Value>> {
        match *key {
            Key::BlockHeader(hash) => if let Some(header) = self.block_header.lock().get_mut(&hash)
            {
                return Ok(Some(Value::BlockHeader(Box::new(header.clone()))));
            },
            Key::BlockHash(height) => if let Some(hash) = self.block_hash.lock().get_mut(&height) {
                return Ok(Some(Value::BlockHash(*hash)));
            },
            _ => return self.db.read(key),
        }
        self.db.read(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::{H520, U256};
    use core::block::RawHeader;
    use core::proof::Proof;

    struct DummyDB {}

    impl KeyValueDB for DummyDB {
        fn write(&self, _batch: Batch) -> Result<()> {
            Ok(())
        }

        fn read(&self, _key: &Key) -> Result<Option<Value>> {
            Ok(None)
        }
    }

    #[test]
    fn write_and_read() {
        let db = CacheKeyValueDB::new(DummyDB {});

        let header = Header::new(
            RawHeader {
                pre_hash: H256::from(0),
                timestamp: 0,
                transactions_root: H256::from(0),
                difficulty: U256::from(0),
                challenge: H256::from(0),
                proof: Proof::default(),
                height: 0,
            },
            U256::from(0),
            Some(H520::from(0)),
        );

        let mut batch = Batch::default();
        batch.insert(KeyValue::BlockHeader(
            header.hash(),
            Box::new(header.clone()),
        ));
        db.write(batch).expect("db operation should be fine");

        assert_eq!(
            Value::BlockHeader(Box::new(header.clone())),
            db.read(&Key::BlockHeader(header.hash())).unwrap().unwrap()
        );

        let mut batch = Batch::default();
        batch.insert(KeyValue::BlockHash(1, H256::from(1)));
        db.write(batch).expect("db operation should be fine");

        assert_eq!(
            Value::BlockHash(H256::from(1)),
            db.read(&Key::BlockHash(1)).unwrap().unwrap()
        );
    }
}
