use crate::pool::TxPool;
use futures::future::Future;
use std::ops::DerefMut;
use std::sync::{atomic::AtomicU64, Arc};
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct ClearPoolProcess {
    pub tx_pool: Lock<TxPool>,
}

impl ClearPoolProcess {
    pub fn new(tx_pool: Lock<TxPool>) -> ClearPoolProcess {
        ClearPoolProcess { tx_pool }
    }
}

impl Future for ClearPoolProcess {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(mut guard) => {
                let config = guard.config;
                let snapshot = Arc::clone(&guard.snapshot);
                let last_txs_updated_at = Arc::new(AtomicU64::new(0));

                let mut new_pool = TxPool::new(config, snapshot, last_txs_updated_at);
                let old_pool = guard.deref_mut();
                ::std::mem::swap(old_pool, &mut new_pool);

                Ok(Async::Ready(()))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
