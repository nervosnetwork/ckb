use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ckb_db::RocksDB;
use ckb_db_schema::COLUMN_BLOCK_HEADER;
#[cfg(feature = "stats")]
use ckb_logger::info;
use ckb_metrics::HistogramTimer;
use ckb_types::packed;
#[cfg(feature = "stats")]
use ckb_util::{Mutex, MutexGuard};
use ckb_util::{RwLock, RwLockReadGuard};

use ckb_types::{U256, core::EpochNumberWithFraction, packed::Byte32, prelude::*};

use super::MemoryMap;
use crate::types::HeaderIndexView;

pub(crate) struct HeaderMapKernel {
    pub(crate) memory: MemoryMap,
    db: RocksDB,
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
}

impl HeaderMapKernel {
    pub(crate) fn new(db: RocksDB, memory_limit: usize, ibd_finished: Arc<AtomicBool>) -> Self {
        let memory = Default::default();
        let shared_best_header_value = Self::default_shared_best_header();
        let shared_best_header = RwLock::new(shared_best_header_value);

        #[cfg(not(feature = "stats"))]
        {
            Self {
                db,
                memory,
                memory_limit,
                ibd_finished,
                shared_best_header,
            }
        }

        #[cfg(feature = "stats")]
        {
            Self {
                db,
                memory,
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
        let contains = self
            .db
            .get_pinned(COLUMN_BLOCK_HEADER, hash.as_slice())
            .unwrap()
            .is_some();

        contains
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

        self.db
            .get_pinned(COLUMN_BLOCK_HEADER, hash.as_slice())
            .unwrap()
            .and_then(|slice| {
                let reader = packed::HeaderViewReader::from_slice_should_be_ok(slice.as_ref());

                let header_view = Unpack::<ckb_types::core::HeaderView>::unpack(&reader);
                Some(header_view)
            });

        None
    }

    pub(crate) fn insert(
        &self,
        view: ckb_types::core::HeaderView,
        total_difficulty: U256,
    ) -> Option<()> {
        #[cfg(feature = "stats")]
        {
            self.trace();
            self.stats().tick_primary_insert();
        }
        let packed_header: packed::HeaderView = view.clone().into();
        let view: HeaderIndexView = (view, total_difficulty).into();
        let hash = view.hash();
        self.memory.insert(view.into());

        self.db
            .put(
                COLUMN_BLOCK_HEADER,
                hash.as_slice(),
                packed_header.as_slice(),
            )
            .ok()
    }

    pub(crate) fn remove(&self, _hash: &Byte32) {
        // TODO
        // no need to remove
    }

    pub(crate) fn limit_memory(&self) {
        let _trace_timer: Option<HistogramTimer> = ckb_metrics::handle()
            .map(|handle| handle.ckb_header_map_limit_memory_duration.start_timer());

        if let Some(values) = self.memory.excess_items(self.memory_limit) {
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
            ",
                self.memory.len(),
                self.memory_limit,
                stats.primary_contain,
                stats.primary_select,
                stats.primary_insert,
                stats.primary_delete,
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

    fn tick_primary_select(&mut self) {
        self.primary_select += 1;
    }

    fn tick_primary_insert(&mut self) {
        self.primary_insert += 1;
    }

    fn tick_primary_delete(&mut self) {
        self.primary_delete += 1;
    }
}
