use crate::types::HeaderView;
use ckb_async_runtime::Handle;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_types::packed::Byte32;
use std::path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::MissedTickBehavior;

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
    stop: StopHandler<()>,
}

impl Drop for HeaderMap {
    fn drop(&mut self) {
        self.stop.try_send(());
    }
}

const INTERVAL: Duration = Duration::from_millis(500);

impl HeaderMap {
    pub(crate) fn new<P>(tmpdir: Option<P>, primary_limit: usize, async_handle: &Handle) -> Self
    where
        P: AsRef<path::Path>,
    {
        let inner = Arc::new(HeaderMapKernel::new(tmpdir, primary_limit));
        let map = Arc::clone(&inner);
        let (stop, mut stop_rx) = oneshot::channel::<()>();

        async_handle.spawn(async move {
            let mut interval = tokio::time::interval(INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        map.limit_memory();
                    }
                    _ = &mut stop_rx => break,
                }
            }
        });

        Self {
            inner,
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
        self.inner.remove(hash)
    }
}
