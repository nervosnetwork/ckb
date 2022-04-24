use crate::types::HeaderView;
use ckb_async_runtime::Handle;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_types::packed::Byte32;
use std::path;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::MissedTickBehavior;
use tokio::sync::{mpsc, oneshot};

mod backend;
mod backend_sled;
mod kernel_lru;
mod memory;

pub(crate) use self::{
    backend::KeyValueBackend, backend_sled::SledBackend, kernel_lru::HeaderMapKernel,
    memory::MemoryMap,
};

pub struct HeaderMap {
    inner: Arc<HeaderMapKernel<SledBackend>>,
    // tx: mpsc::Sender<Byte32>,
    stop: StopHandler<()>,
}

impl Drop for HeaderMap {
    fn drop(&mut self) {
        self.stop.try_send(());
    }
}

impl HeaderMap {
    pub(crate) fn new<P>(tmpdir: Option<P>, primary_limit: usize, async_handle: &Handle) -> Self
    where
        P: AsRef<path::Path>,
    {
        let inner = Arc::new(HeaderMapKernel::new(tmpdir, primary_limit));
        let map = Arc::clone(&inner);
        let interval = Duration::from_millis(500);
        let (stop, mut stop_rx) = oneshot::channel::<()>();
        // let (tx, mut rx) = mpsc::channel(10000);

        async_handle.spawn(async move {
            let mut interval = tokio::time::interval(interval);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            // let mut removed = Vec::with_capacity(1000);
            loop {
                tokio::select! {
                    // Some(key) = rx.recv() => {
                    //     removed.push(key);
                    // }
                    _ = interval.tick() => {
                        // map.backend_remove_batch(removed.drain(..).collect());
                        map.limit_memory();
                    }
                    _ = &mut stop_rx => break,
                }
            }
        });

        Self {
            inner,
            // tx,
            stop: StopHandler::new(SignalSender::Tokio(stop), None, "HeaderMap".to_string()),
        }
    }

    pub(crate) fn contains_key(&self, hash: &Byte32) -> bool {
        self.inner.contains_key(hash)
    }

    pub(crate) fn get(&self, hash: &Byte32) -> Option<HeaderView> {
        self.inner.get(hash)
    }

    pub(crate) fn insert(&self, view: HeaderView) -> Option<()> {
        self.inner.insert(view)
    }

    pub(crate) fn remove(&self, hash: &Byte32) {
        self.inner.memory_remove(hash);
        if self.inner.backend.is_empty() {
            return;
        }
        self.inner.backend.remove_no_return(hash);
    }
}
