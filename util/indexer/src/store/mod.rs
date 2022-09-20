mod rocksdb;
mod secondary_db;

pub(crate) use self::rocksdb::RocksdbStore;
pub(crate) use self::secondary_db::SecondaryDB;
use crate::error::Error;
use std::path::Path;

type IteratorItem = (Box<[u8]>, Box<[u8]>);

pub(crate) enum IteratorDirection {
    Forward,
    Reverse,
}

pub(crate) trait Store {
    type Batch: Batch;

    fn new<P>(path: P) -> Self
    where
        P: AsRef<Path>;

    fn get<K: AsRef<[u8]>>(&self, key: K) -> Result<Option<Vec<u8>>, Error>;

    fn exists<K: AsRef<[u8]>>(&self, key: K) -> Result<bool, Error>;

    fn iter<K: AsRef<[u8]>>(
        &self,
        from_key: K,
        direction: IteratorDirection,
    ) -> Result<Box<dyn Iterator<Item = IteratorItem> + '_>, Error>;

    fn batch(&self) -> Result<Self::Batch, Error>;
}

pub(crate) trait Batch {
    fn put_kv<K: Into<Vec<u8>>, V: Into<Vec<u8>>>(
        &mut self,
        key: K,
        value: V,
    ) -> Result<(), Error> {
        self.put(&Into::<Vec<u8>>::into(key), &Into::<Vec<u8>>::into(value))
    }

    fn put<K: AsRef<[u8]>, V: AsRef<[u8]>>(&mut self, key: K, value: V) -> Result<(), Error>;
    fn delete<K: AsRef<[u8]>>(&mut self, key: K) -> Result<(), Error>;
    fn commit(self) -> Result<(), Error>;
}
