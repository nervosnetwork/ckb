use std::{clone, cmp, default, hash};

use ckb_util::shrink_to_fit;
use ckb_util::LinkedHashMap;

use crate::types::SHRINK_THRESHOLD;

pub(crate) struct KeyValueMemory<K, V>(LinkedHashMap<K, V>)
where
    K: cmp::Eq + hash::Hash;

impl<K, V> default::Default for KeyValueMemory<K, V>
where
    K: cmp::Eq + hash::Hash,
{
    fn default() -> Self {
        Self(default::Default::default())
    }
}

impl<K, V> KeyValueMemory<K, V>
where
    K: cmp::Eq + hash::Hash,
    V: clone::Clone,
{
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn contains_key(&self, key: &K) -> bool {
        self.0.contains_key(key)
    }

    pub(crate) fn get_refresh(&mut self, key: &K) -> Option<V> {
        self.0.get_refresh(key).cloned()
    }

    pub(crate) fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.0.insert(key, value)
    }

    pub(crate) fn remove(&mut self, key: &K) -> Option<V> {
        let ret = self.0.remove(key);
        shrink_to_fit!(self.0, SHRINK_THRESHOLD);
        ret
    }

    pub(crate) fn pop_front(&mut self) -> Option<(K, V)> {
        let ret = self.0.pop_front();
        shrink_to_fit!(self.0, SHRINK_THRESHOLD);
        ret
    }
}
