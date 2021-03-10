use super::{Key, Value};

pub(crate) trait StorageBackend {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn is_opened(&self) -> bool;
    fn open(&self);
    fn try_close(&self) -> bool;
}

pub(crate) trait KeyValueBackend<K: Key, V: Value>: StorageBackend {
    fn contains_key(&self, key: &K) -> bool;
    fn get(&self, key: &K) -> Option<V>;
    fn insert(&mut self, key: &K, value: &V) -> Option<V>;
    fn remove(&mut self, key: &K) -> Option<V>;
}
