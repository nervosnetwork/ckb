use ckb_types::{core::Cycle, packed::Byte32};
pub use ckb_verification::txs_verify_cache::{FetchCache, UpdateCache};
use futures::Future;
use lru_cache::LruCache;
use std::collections::HashMap;
use std::mem;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::{Lock, LockGuard};

#[derive(Debug)]
pub enum MaybeAcquired<A> {
    NotYet(Lock<A>),
    Acquired(LockGuard<A>),
    Gone,
}

impl<A> MaybeAcquired<A> {
    pub fn poll(&mut self) -> bool {
        match *self {
            MaybeAcquired::NotYet(ref mut a) => match a.poll_lock() {
                Async::Ready(guard) => {
                    *self = MaybeAcquired::Acquired(guard);
                    true
                }
                Async::NotReady => false,
            },
            MaybeAcquired::Acquired(_) => return true,
            MaybeAcquired::Gone => panic!("cannot poll_lock twice"),
        }
    }

    pub fn take(&mut self) -> LockGuard<A> {
        match mem::replace(self, MaybeAcquired::Gone) {
            MaybeAcquired::Acquired(guard) => guard,
            _ => panic!(),
        }
    }
}
