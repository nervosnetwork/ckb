use std::path;

use ckb_types::{packed::Byte32, prelude::Entity};
use ckb_util::Mutex;

use crate::types::HeaderView;

mod backend;
mod backend_rocksdb;
mod kernel_lru;
mod keyvalue;
mod memory;

pub(crate) use self::{
    backend::{KeyValueBackend, StorageBackend},
    backend_rocksdb::RocksDBBackend,
    kernel_lru::HashMapLruKernel,
    keyvalue::{Key, Value},
    memory::KeyValueMemory,
};

pub(crate) type HeaderMapLru = HashMapLru<Byte32, HeaderView, RocksDBBackend>;

pub(crate) struct HashMapLru<K: Key, V: Value, B: KeyValueBackend<K, V>>(
    Mutex<HashMapLruKernel<K, V, B>>,
);

impl Key for Byte32 {
    fn as_slice(&self) -> &[u8] {
        Entity::as_slice(self)
    }
}

impl Value for HeaderView {
    fn from_slice(slice: &[u8]) -> Self {
        Self::from_slice_should_be_ok(&slice)
    }

    fn to_vec(&self) -> Vec<u8> {
        self.to_vec()
    }
}

impl<K, V, B> HashMapLru<K, V, B>
where
    K: Key,
    V: Value,
    B: KeyValueBackend<K, V>,
{
    pub(crate) fn new<P>(
        tmpdir: Option<P>,
        primary_limit: usize,
        backend_close_threshold: usize,
    ) -> Self
    where
        P: AsRef<path::Path>,
    {
        let inner = HashMapLruKernel::new(tmpdir, primary_limit, backend_close_threshold);
        Self(Mutex::new(inner))
    }

    pub(crate) fn contains_key(&self, hash: &K) -> bool {
        self.0.lock().contains_key(hash)
    }

    pub(crate) fn get(&self, hash: &K) -> Option<V> {
        self.0.lock().get(hash)
    }

    pub(crate) fn insert(&self, hash: K, view: V) -> Option<V> {
        self.0.lock().insert(hash, view)
    }

    pub(crate) fn remove(&self, hash: &K) -> Option<V> {
        self.0.lock().remove(hash)
    }
}
