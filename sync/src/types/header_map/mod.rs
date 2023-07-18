use ckb_async_runtime::Handle;
use ckb_logger::debug;
use ckb_stop_handler::{new_tokio_exit_rx, CancellationToken};
use ckb_types::packed::Byte32;
use std::sync::Arc;
use std::time::Duration;
use std::{mem::size_of, path};

use tokio::time::MissedTickBehavior;

mod backend;
mod backend_sled;
mod kernel_lru;
mod memory;

pub(crate) use self::{
    backend::KeyValueBackend, backend_sled::SledBackend, kernel_lru::HeaderMapKernel,
    memory::MemoryMap,
};

use super::HeaderIndexView;

pub struct HeaderMap {
    inner: Arc<HeaderMapKernel<SledBackend>>,
}

const INTERVAL: Duration = Duration::from_millis(500);
const ITEM_BYTES_SIZE: usize = size_of::<HeaderIndexView>();
const WARN_THRESHOLD: usize = ITEM_BYTES_SIZE * 100_000;

impl HeaderMap {
    pub(crate) fn new<P>(tmpdir: Option<P>, memory_limit: usize, async_handle: &Handle) -> Self
    where
        P: AsRef<path::Path>,
    {
        if memory_limit < ITEM_BYTES_SIZE {
            panic!("The limit setting is too low");
        }
        if memory_limit < WARN_THRESHOLD {
            ckb_logger::warn!(
                "The low memory limit setting {} will result in inefficient synchronization",
                memory_limit
            );
        }
        let size_limit = memory_limit / ITEM_BYTES_SIZE;
        let inner = Arc::new(HeaderMapKernel::new(tmpdir, size_limit));
        let map = Arc::clone(&inner);
        let stop_rx: CancellationToken = new_tokio_exit_rx();

        async_handle.spawn(async move {
            let mut interval = tokio::time::interval(INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        map.limit_memory();
                    }
                    _ = stop_rx.cancelled() => {
                        debug!("HeaderMap limit_memory received exit signal, exit now");
                        break
                    },
                }
            }
        });

        Self { inner }
    }

    pub(crate) fn contains_key(&self, hash: &Byte32) -> bool {
        self.inner.contains_key(hash)
    }

    pub(crate) fn get(&self, hash: &Byte32) -> Option<HeaderIndexView> {
        self.inner.get(hash)
    }

    pub(crate) fn insert(&self, view: HeaderIndexView) -> Option<()> {
        self.inner.insert(view)
    }

    pub(crate) fn remove(&self, hash: &Byte32) {
        self.inner.remove(hash)
    }
}
