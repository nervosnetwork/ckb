use std::path;

#[cfg(feature = "stats")]
use ckb_logger::trace;
#[cfg(feature = "stats")]
use ckb_util::{Mutex, MutexGuard};

use ckb_types::packed::Byte32;

use super::{KeyValueBackend, MemoryMap};
use crate::types::HeaderIndexView;

pub(crate) struct HeaderMapKernel<Backend>
where
    Backend: KeyValueBackend,
{
    pub(crate) memory: MemoryMap,
    pub(crate) backend: Backend,
    // Configuration
    memory_limit: usize,
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

impl<Backend> HeaderMapKernel<Backend>
where
    Backend: KeyValueBackend,
{
    pub(crate) fn new<P>(tmpdir: Option<P>, memory_limit: usize) -> Self
    where
        P: AsRef<path::Path>,
    {
        let memory = Default::default();
        let backend = Backend::new(tmpdir);

        #[cfg(not(feature = "stats"))]
        {
            Self {
                memory,
                backend,
                memory_limit,
            }
        }

        #[cfg(feature = "stats")]
        {
            Self {
                memory,
                backend,
                memory_limit,
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
            return true;
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
            return Some(view);
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
        self.memory.remove(hash);
        if self.backend.is_empty() {
            return;
        }
        self.backend.remove_no_return(hash);
    }

    pub(crate) fn limit_memory(&self) {
        if let Some(values) = self.memory.front_n(self.memory_limit) {
            tokio::task::block_in_place(|| {
                self.backend.insert_batch(&values);
            });
            self.memory
                .remove_batch(values.iter().map(|value| value.hash()));
        }
    }

    #[cfg(feature = "stats")]
    fn trace(&self) {
        let mut stats = self.stats();
        let progress = stats.trace_progress();
        let frequency = stats.frequency();
        if progress % frequency == 0 {
            trace!(
                "Header Map Statistics\
            \n>\t| storage | length  |  limit  | contain |   select   | insert  | delete  |\
            \n>\t|---------+---------+---------+---------+------------+---------+---------|\
            \n>\t| memory |{:>9}|{:>9}|{:>9}|{:>12}|{:>9}|{:>9}|\
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
