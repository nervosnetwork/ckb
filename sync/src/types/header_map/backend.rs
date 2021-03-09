use std::path;

use super::{Key, Value};

pub(crate) trait StorageBackend {
    fn new<P>(tmpdir: Option<P>) -> Self
    where
        P: AsRef<path::Path>;

    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn is_opened(&self) -> bool;
    fn open(&mut self);
    fn try_close(&mut self) -> bool;
}

pub(crate) trait KeyValueBackend<K: Key, V: Value>: StorageBackend {
    fn contains_key(&self, key: &K) -> bool;
    fn get(&self, key: &K) -> Option<V>;
    fn insert(&mut self, key: &K, value: &V) -> Option<V>;
    fn remove(&mut self, key: &K) -> Option<V>;
}
