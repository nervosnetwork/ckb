use ckb_async_runtime::Handle;
use ckb_logger::info;
use ckb_stop_handler::{CancellationToken, new_tokio_exit_rx};
use ckb_types::packed::Byte32;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use std::{mem::size_of, path};

use ckb_metrics::HistogramTimer;
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

const INTERVAL: Duration = Duration::from_millis(5000);
const ITEM_BYTES_SIZE: usize = size_of::<HeaderIndexView>();
const WARN_THRESHOLD: usize = ITEM_BYTES_SIZE * 100_000;

impl HeaderMap {
    pub fn new<P>(
        tmpdir: Option<P>,
        memory_limit: usize,
        async_handle: &Handle,
        ibd_finished: Arc<AtomicBool>,
    ) -> Self
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
        let inner = Arc::new(HeaderMapKernel::new(tmpdir, size_limit, ibd_finished));
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
                        info!("HeaderMap limit_memory received exit signal, exit now");
                        break
                    },
                }
            }
        });

        Self { inner }
    }

    pub fn contains_key(&self, hash: &Byte32) -> bool {
        let _trace_timer: Option<HistogramTimer> = ckb_metrics::handle().map(|metric| {
            metric
                .ckb_header_map_ops_duration
                .with_label_values(&["contains_key"])
                .start_timer()
        });

        self.inner.contains_key(hash)
    }

    pub fn get(&self, hash: &Byte32) -> Option<HeaderIndexView> {
        let _trace_timer: Option<HistogramTimer> = ckb_metrics::handle().map(|metric| {
            metric
                .ckb_header_map_ops_duration
                .with_label_values(&["get"])
                .start_timer()
        });
        self.inner.get(hash)
    }

    pub fn insert(&self, view: HeaderIndexView) -> Option<()> {
        let _trace_timer: Option<HistogramTimer> = ckb_metrics::handle().map(|metric| {
            metric
                .ckb_header_map_ops_duration
                .with_label_values(&["insert"])
                .start_timer()
        });

        self.inner.insert(view)
    }

    pub fn remove(&self, hash: &Byte32) {
        let _trace_timer: Option<HistogramTimer> = ckb_metrics::handle().map(|metric| {
            metric
                .ckb_header_map_ops_duration
                .with_label_values(&["remove"])
                .start_timer()
        });

        self.inner.remove(hash)
    }
}
