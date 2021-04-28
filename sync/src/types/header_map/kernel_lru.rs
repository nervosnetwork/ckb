use std::path;

#[cfg(feature = "stats")]
use ckb_logger::trace;
use ckb_types::packed::Byte32;

use super::{KeyValueBackend, KeyValueMemory};
use crate::types::HeaderView;

pub(crate) struct HeaderMapLruKernel<Backend>
where
    Backend: KeyValueBackend,
{
    primary: KeyValueMemory<Byte32, HeaderView>,
    backend: Backend,
    // Configuration
    primary_limit: usize,
    backend_close_threshold: usize,
    // Statistics
    #[cfg(feature = "stats")]
    stats: HeaderMapLruKernelStats,
}

#[cfg(feature = "stats")]
#[derive(Default)]
struct HeaderMapLruKernelStats {
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

impl<Backend> HeaderMapLruKernel<Backend>
where
    Backend: KeyValueBackend,
{
    pub(crate) fn new<P>(
        tmpdir: Option<P>,
        primary_limit: usize,
        backend_close_threshold: usize,
    ) -> Self
    where
        P: AsRef<path::Path>,
    {
        let primary = Default::default();
        let backend = Backend::new(tmpdir);

        #[cfg(not(feature = "stats"))]
        {
            Self {
                primary,
                backend,
                primary_limit,
                backend_close_threshold,
            }
        }

        #[cfg(feature = "stats")]
        {
            Self {
                primary,
                backend,
                primary_limit,
                backend_close_threshold,
                stats: HeaderMapLruKernelStats::new(50_000),
            }
        }
    }

    pub(crate) fn contains_key(&mut self, hash: &Byte32) -> bool {
        #[cfg(feature = "stats")]
        {
            self.mut_stats().tick_primary_contain();
        }
        if self.primary.contains_key(hash) {
            return true;
        }
        if !self.backend.is_opened() {
            return false;
        }
        #[cfg(feature = "stats")]
        {
            self.mut_stats().tick_backend_contain();
        }
        self.backend.contains_key(hash)
    }

    pub(crate) fn get(&mut self, hash: &Byte32) -> Option<HeaderView> {
        #[cfg(feature = "stats")]
        {
            self.mut_stats().tick_primary_select();
        }
        if let Some(view) = self.primary.get_refresh(hash) {
            return Some(view);
        }
        if !self.backend.is_opened() {
            return None;
        }
        #[cfg(feature = "stats")]
        {
            self.mut_stats().tick_backend_delete();
        }
        if let Some(view) = self.backend.remove(hash) {
            if self.primary.len() >= self.primary_limit {
                #[cfg(feature = "stats")]
                {
                    self.mut_stats().tick_primary_delete();
                    self.mut_stats().tick_backend_insert();
                }
                if let Some((_, view_old)) = self.primary.pop_front() {
                    self.backend.insert(&view_old);
                }
            } else if self.primary.len() < self.backend_close_threshold {
                self.backend.try_close();
            }
            #[cfg(feature = "stats")]
            {
                self.mut_stats().tick_primary_insert();
            }
            self.primary.insert(view.hash(), view.clone());
            Some(view)
        } else {
            None
        }
    }

    pub(crate) fn insert(&mut self, view: HeaderView) -> Option<HeaderView> {
        #[cfg(feature = "stats")]
        {
            self.trace();
            self.mut_stats().tick_primary_insert();
        }
        if let Some(view) = self.primary.insert(view.hash(), view.clone()) {
            return Some(view);
        }
        let view_opt = self.backend.remove(&view.hash());
        if self.primary.len() > self.primary_limit {
            self.backend.open();
            #[cfg(feature = "stats")]
            {
                self.mut_stats().tick_primary_delete();
                self.mut_stats().tick_backend_insert();
            }
            if let Some((_, view_old)) = self.primary.pop_front() {
                self.backend.insert(&view_old);
            }
        }
        view_opt
    }

    pub(crate) fn remove(&mut self, hash: &Byte32) -> Option<HeaderView> {
        #[cfg(feature = "stats")]
        {
            self.trace();
            self.mut_stats().tick_primary_delete();
        }
        if let Some(view) = self.primary.remove(hash) {
            return Some(view);
        }
        if !self.backend.is_opened() {
            return None;
        }
        #[cfg(feature = "stats")]
        {
            self.mut_stats().tick_backend_delete();
        }
        let view_opt = self.backend.remove(hash);
        if self.primary.len() < self.backend_close_threshold {
            self.backend.try_close();
        }
        view_opt
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
            \n>\t| primary |{:>9}|{:>9}|{:>9}|{:>12}|{:>9}|{:>9}|\
            \n>\t| backend |{:>9}|{:>9}|{:>9}|{:>12}|{:>9}|{:>9}|\
            ",
                self.primary.len(),
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
