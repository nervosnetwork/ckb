use std::path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ckb_logger::info;
use ckb_metrics::HistogramTimer;
#[cfg(feature = "stats")]
use ckb_util::{Mutex, MutexGuard};
use ckb_util::{RwLock, RwLockReadGuard};

use ckb_types::{U256, core::EpochNumberWithFraction, packed::Byte32};

use super::{MemoryMap, SledBackend};
use crate::types::HeaderIndexView;

pub(crate) struct HeaderMapKernel {
    pub(crate) memory: MemoryMap,
    pub(crate) backend: SledBackend,
    // Configuration
    memory_limit: usize,
    // if ckb is in IBD mode, don't shrink memory map
    ibd_finished: Arc<AtomicBool>,
    shared_best_header: RwLock<HeaderIndexView>,
    // Statistics
    #[cfg(feature = "stats")]
    stats: Mutex<HeaderMapKernelStats>,
}

#[cfg(feature = "stats")]
#[derive(Default)]
struct HeaderMapKernelStats {
    frequency: usize,

    trace_progress: usize,

    primary_contain: usize,
    primary_select: usize,
    primary_insert: usize,
    primary_delete: usize,

    backend_contain: usize,
    backend_delete: usize,
}

impl Drop for HeaderMapKernel {
    fn drop(&mut self) {
        loop {
            let items = self.memory.front_items(1024);
            if items.is_empty() {
                break;
            }

            self.backend.insert_batch(&items);
            self.memory
                .remove_batch(items.iter().map(|item| item.hash()), false);
        }
        let best_header = self.shared_best_header.read().clone();
        self.backend.store_shared_best_header(&best_header);
        info!("HeaderMap persisted all items to backend");
    }
}

impl HeaderMapKernel {
    pub(crate) fn new<P>(
        tmpdir: Option<P>,
        memory_limit: usize,
        ibd_finished: Arc<AtomicBool>,
    ) -> Self
    where
        P: AsRef<path::Path>,
    {
        let memory = Default::default();
        let backend = SledBackend::new(tmpdir);
        info!("backend is empty: {}", backend.is_empty());
        let shared_best_header_value = backend
            .load_shared_best_header()
            .unwrap_or_else(Self::default_shared_best_header);
        let shared_best_header = RwLock::new(shared_best_header_value);

        #[cfg(not(feature = "stats"))]
        {
            Self {
                memory,
                backend,
                memory_limit,
                ibd_finished,
                shared_best_header,
            }
        }

        #[cfg(feature = "stats")]
        {
            Self {
                memory,
                backend,
                memory_limit,
                ibd_finished,
                shared_best_header,
                stats: Mutex::new(HeaderMapKernelStats::new(50_000)),
            }
        }
    }

    pub(crate) fn contains_key(&self, hash: &Byte32) -> bool {
        #[cfg(feature = "stats")]
        {
            self.stats().tick_primary_contain();
        }
        if self.memory.contains_key(hash) {
            if let Some(metrics) = ckb_metrics::handle() {
                metrics.ckb_header_map_memory_hit_miss_count.hit.inc()
            }
            return true;
        }
        if let Some(metrics) = ckb_metrics::handle() {
            metrics.ckb_header_map_memory_hit_miss_count.miss.inc();
        }

        if self.backend.is_empty() {
            return false;
        }
        #[cfg(feature = "stats")]
        {
            self.stats().tick_backend_contain();
        }
        self.backend.contains_key(hash)
    }

    pub(crate) fn get(&self, hash: &Byte32) -> Option<HeaderIndexView> {
        #[cfg(feature = "stats")]
        {
            self.stats().tick_primary_select();
        }
        if let Some(view) = self.memory.get_refresh(hash) {
            if let Some(metrics) = ckb_metrics::handle() {
                metrics.ckb_header_map_memory_hit_miss_count.hit.inc();
            }
            return Some(view);
        }

        if let Some(metrics) = ckb_metrics::handle() {
            metrics.ckb_header_map_memory_hit_miss_count.miss.inc();
        }

        if self.backend.is_empty() {
            return None;
        }
        #[cfg(feature = "stats")]
        {
            self.stats().tick_backend_delete();
        }
        if let Some(view) = self.backend.remove(hash) {
            #[cfg(feature = "stats")]
            {
                self.stats().tick_primary_insert();
            }
            self.memory.insert(view.clone());
            Some(view)
        } else {
            None
        }
    }

    pub(crate) fn insert(&self, view: HeaderIndexView) -> Option<()> {
        #[cfg(feature = "stats")]
        {
            self.trace();
            self.stats().tick_primary_insert();
        }
        self.memory.insert(view)
    }

    pub(crate) fn remove(&self, hash: &Byte32) {
        #[cfg(feature = "stats")]
        {
            self.trace();
            self.stats().tick_primary_delete();
        }
        // If IBD is not finished, don't shrink memory map
        let allow_shrink_to_fit = self.ibd_finished.load(Ordering::Acquire);
        self.memory.remove(hash, allow_shrink_to_fit);
        if self.backend.is_empty() {
            return;
        }
        self.backend.remove_no_return(hash);
    }

    pub(crate) fn limit_memory(&self) {
        let _trace_timer: Option<HistogramTimer> = ckb_metrics::handle()
            .map(|handle| handle.ckb_header_map_limit_memory_duration.start_timer());

        if let Some(values) = self.memory.excess_items(self.memory_limit) {
            tokio::task::block_in_place(|| {
                self.backend.insert_batch(&values);
            });

            // If IBD is not finished, don't shrink memory map
            let allow_shrink_to_fit = self.ibd_finished.load(Ordering::Acquire);
            self.memory
                .remove_batch(values.iter().map(|value| value.hash()), allow_shrink_to_fit);
        }
    }

    pub(crate) fn shared_best_header(&self) -> HeaderIndexView {
        self.shared_best_header.read().clone()
    }

    pub(crate) fn shared_best_header_ref(&self) -> RwLockReadGuard<HeaderIndexView> {
        self.shared_best_header.read()
    }

    pub(crate) fn set_shared_best_header(&self, header: HeaderIndexView) {
        if let Some(metrics) = ckb_metrics::handle() {
            metrics.ckb_shared_best_number.set(header.number() as i64);
        }
        *self.shared_best_header.write() = header;
    }

    pub(crate) fn may_set_shared_best_header(&self, header: HeaderIndexView) {
        {
            let current = self.shared_best_header.read();
            if !header.is_better_than(current.total_difficulty()) {
                return;
            }
        }

        self.set_shared_best_header(header);
    }

    fn default_shared_best_header() -> HeaderIndexView {
        HeaderIndexView::new(
            Byte32::zero(),
            0,
            EpochNumberWithFraction::from_full_value(0),
            0,
            Byte32::default(),
            U256::zero(),
        )
    }

    #[cfg(feature = "stats")]
    fn trace(&self) {
        let mut stats = self.stats();
        let progress = stats.trace_progress();
        let frequency = stats.frequency();
        if progress % frequency == 0 {
            info!(
                "Header Map Statistics\
            \n>\t| storage | length  |  limit  | contain |   select   | insert  | delete  |\
            \n>\t|---------+---------+---------+---------+------------+---------+---------|\
            \n>\t| memory  |{:>9}|{:>9}|{:>9}|{:>12}|{:>9}|{:>9}|\
            \n>\t| backend |{:>9}|{:>9}|{:>9}|{:>12}|{:>9}|{:>9}|\
            ",
                self.memory.len(),
                self.memory_limit,
                stats.primary_contain,
                stats.primary_select,
                stats.primary_insert,
                stats.primary_delete,
                self.backend.len(),
                '-',
                stats.backend_contain,
                '-',
                '-',
                stats.backend_delete,
            );
            stats.trace_progress_reset();
        } else {
            stats.trace_progress_tick();
        }
    }

    #[cfg(feature = "stats")]
    fn stats(&self) -> MutexGuard<HeaderMapKernelStats> {
        self.stats.lock()
    }
}

#[cfg(feature = "stats")]
impl HeaderMapKernelStats {
    fn new(frequency: usize) -> Self {
        Self {
            frequency,
            ..Default::default()
        }
    }

    fn frequency(&self) -> usize {
        self.frequency
    }

    fn trace_progress(&self) -> usize {
        self.trace_progress
    }

    fn trace_progress_reset(&mut self) {
        self.trace_progress = 1;
    }

    fn trace_progress_tick(&mut self) {
        self.trace_progress += 1;
    }

    fn tick_primary_contain(&mut self) {
        self.primary_contain += 1;
    }

    fn tick_backend_contain(&mut self) {
        self.backend_contain += 1;
    }

    fn tick_primary_select(&mut self) {
        self.primary_select += 1;
    }

    fn tick_primary_insert(&mut self) {
        self.primary_insert += 1;
    }

    fn tick_primary_delete(&mut self) {
        self.primary_delete += 1;
    }

    fn tick_backend_delete(&mut self) {
        self.backend_delete += 1;
    }
}
