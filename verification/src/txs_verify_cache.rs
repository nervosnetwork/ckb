use ckb_types::{core::Cycle, packed::Byte32};
use futures::Future;
use lru_cache::LruCache;
use std::collections::HashMap;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::{Lock};

pub struct FetchCache<K: IntoIterator<Item = Byte32>> {
    cache: Lock<LruCache<Byte32, Cycle>>,
    keys: Option<K>,
}

impl<K: IntoIterator<Item = Byte32>> FetchCache<K> {
    pub fn new(cache: Lock<LruCache<Byte32, Cycle>>, keys: K) -> FetchCache<K> {
        FetchCache {
            cache,
            keys: Some(keys),
        }
    }
}

impl<K: IntoIterator<Item = Byte32>> Future for FetchCache<K> {
    type Item = HashMap<Byte32, Cycle>;
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.cache.poll_lock() {
            Async::Ready(guard) => {
                let keys = self.keys.take().expect("cannot poll twice");
                Ok(Async::Ready(
                    keys.into_iter()
                        .filter_map(|key| guard.get(&key).cloned().map(|value| (key, value)))
                        .collect(),
                ))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}

pub struct UpdateCache {
    cache: Lock<LruCache<Byte32, Cycle>>,
    map: Option<HashMap<Byte32, Cycle>>,
}

impl UpdateCache {
    pub fn new(cache: Lock<LruCache<Byte32, Cycle>>, map: HashMap<Byte32, Cycle>) -> UpdateCache {
        UpdateCache {
            cache,
            map: Some(map),
        }
    }
}

impl Future for UpdateCache {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.cache.poll_lock() {
            Async::Ready(mut guard) => {
                let map = self.map.take().expect("cannot poll twice");
                for (k, v) in map {
                    guard.insert(k, v);
                }
                Ok(Async::Ready(()))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
