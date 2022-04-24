use std::path;

#[cfg(feature = "stats")]
use ckb_logger::trace;
use ckb_types::packed::Byte32;

use super::{KeyValueBackend, MemoryMap};
use crate::types::HeaderView;

pub(crate) struct HeaderMapKernel<Backend>
where
    Backend: KeyValueBackend,
{
    pub(crate) memory: MemoryMap,
    pub(crate) backend: Backend,
    // Configuration
    primary_limit: usize,
    // Statistics
    #[cfg(feature = "stats")]
    stats: HeaderMapKernelStats,
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
    backend_insert: usize,
    backend_delete: usize,
}

impl<Backend> HeaderMapKernel<Backend>
where
    Backend: KeyValueBackend,
{
    pub(crate) fn new<P>(tmpdir: Option<P>, primary_limit: usize) -> Self
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
                primary_limit,
            }
        }

        #[cfg(feature = "stats")]
        {
            Self {
                memory,
                backend,
                primary_limit,
                stats: HeaderMapLruKernelStats::new(50_000),
            }
        }
    }

    pub(crate) fn contains_key(&self, hash: &Byte32) -> bool {
        #[cfg(feature = "stats")]
        {
            self.mut_stats().tick_primary_contain();
        }
        if self.memory.contains_key(hash) {
            return true;
        }
        if self.backend.is_empty() {
            return false;
        }
        #[cfg(feature = "stats")]
        {
            self.mut_stats().tick_backend_contain();
        }
        self.backend.contains_key(hash)
    }

    pub(crate) fn get(&self, hash: &Byte32) -> Option<HeaderView> {
        #[cfg(feature = "stats")]
        {
            self.mut_stats().tick_primary_select();
        }
        if let Some(view) = self.memory.get_refresh(hash) {
            return Some(view);
        }
        if self.backend.is_empty() {
            return None;
        }
        #[cfg(feature = "stats")]
        {
            self.mut_stats().tick_backend_delete();
        }
        if let Some(view) = self.backend.remove(hash) {
            #[cfg(feature = "stats")]
            {
                self.mut_stats().tick_primary_insert();
            }
            self.memory.insert(view.hash(), view.clone());
            Some(view)
        } else {
            None
        }
    }

    pub(crate) fn insert(&self, view: HeaderView) -> Option<()> {
        #[cfg(feature = "stats")]
        {
            self.trace();
            self.mut_stats().tick_primary_insert();
        }
        self.memory.insert(view.hash(), view.clone())
        // let view_opt = self.backend.remove(&view.hash());
        // if self.memory.len() > self.primary_limit {
        //     self.backend.open();
        //     #[cfg(feature = "stats")]
        //     {
        //         self.mut_stats().tick_primary_delete();
        //         self.mut_stats().tick_backend_insert();
        //     }
        //     if let Some((_, view_old)) = self.memory.pop_front() {
        //         self.backend.insert(&view_old);
        //     }
        // }
        // view_opt
    }

    pub(crate) fn memory_remove(&self, hash: &Byte32) {
        #[cfg(feature = "stats")]
        {
            self.trace();
            self.mut_stats().tick_primary_delete();
        }
        self.memory.remove(hash);
    }

    pub(crate) fn limit_memory(&self) {
        if let Some(values) = self.memory.front(self.primary_limit) {
            tokio::task::block_in_place(|| {
                self.backend.insert_batch(&values);
            });
            self.memory
                .remove_batch(values.iter().map(|value| value.hash()));
        }
    }

    pub(crate) fn backend_remove_batch(&self, keys: Vec<Byte32>) {
        if !self.backend.is_empty() {
            self.backend.remove_batch(&keys[..]);
        }
    }

    #[cfg(feature = "stats")]
    fn trace(&mut self) {
        let progress = self.stats().trace_progress();
        let frequency = self.stats().frequency();
        if progress % frequency == 0 {
            trace!(
                "Header Map Statistics\
            \n>\t| storage | length  |  limit  | contain |   select   | insert  | delete  |\
            \n>\t|---------+---------+---------+---------+------------+---------+---------|\
            \n>\t| memory |{:>9}|{:>9}|{:>9}|{:>12}|{:>9}|{:>9}|\
            \n>\t| backend |{:>9}|{:>9}|{:>9}|{:>12}|{:>9}|{:>9}|\
            ",
                self.memory.len(),
                self.primary_limit,
                self.stats().primary_contain,
                self.stats().primary_select,
                self.stats().primary_insert,
                self.stats().primary_delete,
                self.backend.len(),
                self.backend.is_opened(),
                self.stats().backend_contain,
                '-',
                self.stats().backend_insert,
                self.stats().backend_delete,
            );
            self.mut_stats().trace_progress_reset();
        } else {
            self.mut_stats().trace_progress_tick();
        }
    }

    #[cfg(feature = "stats")]
    fn stats(&self) -> &HeaderMapLruKernelStats {
        &self.stats
    }

    #[cfg(feature = "stats")]
    fn mut_stats(&mut self) -> &mut HeaderMapLruKernelStats {
        &mut self.stats
    }
}

#[cfg(feature = "stats")]
impl HeaderMapLruKernelStats {
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

    fn tick_backend_insert(&mut self) {
        self.backend_insert += 1;
    }

    fn tick_primary_delete(&mut self) {
        self.primary_delete += 1;
    }

    fn tick_backend_delete(&mut self) {
        self.backend_delete += 1;
    }
}
