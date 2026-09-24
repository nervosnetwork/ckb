//! Shared process-wide requested-allocation counter for finite benchmarks.
//! Counts alloc/realloc requests, including failed requests and full realloc sizes.
//! This is cumulative traffic, not retained bytes, allocator footprint, or RSS.
//! Windows must not overlap; unrelated process threads can contribute.

#[cfg(feature = "allocation-observation")]
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

#[cfg(feature = "allocation-observation")]
struct CountingAllocator;

#[cfg(feature = "allocation-observation")]
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
#[cfg(feature = "allocation-observation")]
static ALLOCATION_WINDOW_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "allocation-observation")]
static ALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "allocation-observation")]
static ALLOCATION_BYTES: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "allocation-observation")]
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ALLOCATION_WINDOW_ACTIVE.load(Ordering::Acquire) {
            ALLOCATION_CALLS.fetch_add(1, Ordering::AcqRel);
            ALLOCATION_BYTES.fetch_add(layout.size() as u64, Ordering::AcqRel);
        }
        // SAFETY: this allocator delegates every operation to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` and `layout` are forwarded unchanged to their owner.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if ALLOCATION_WINDOW_ACTIVE.load(Ordering::Acquire) {
            ALLOCATION_CALLS.fetch_add(1, Ordering::AcqRel);
            ALLOCATION_BYTES.fetch_add(size as u64, Ordering::AcqRel);
        }
        // SAFETY: the complete reallocation request is delegated unchanged.
        unsafe { System.realloc(pointer, layout, size) }
    }
}

/// Owns the lifetime of one non-overlapping process-wide observation window. Dropping an
/// unfinished window stops counting without publishing a successful sample.
pub(crate) struct AllocationWindow {
    _private: (),
}

impl AllocationWindow {
    pub(crate) fn begin() -> Self {
        #[cfg(feature = "allocation-observation")]
        {
            ALLOCATION_CALLS.store(0, Ordering::Release);
            ALLOCATION_BYTES.store(0, Ordering::Release);
            ALLOCATION_WINDOW_ACTIVE.store(true, Ordering::Release);
        }
        Self { _private: () }
    }

    pub(crate) fn finish(self) -> (u64, u64) {
        drop(self);
        #[cfg(feature = "allocation-observation")]
        {
            (
                ALLOCATION_CALLS.load(Ordering::Acquire),
                ALLOCATION_BYTES.load(Ordering::Acquire),
            )
        }
        #[cfg(not(feature = "allocation-observation"))]
        (0, 0)
    }
}

impl Drop for AllocationWindow {
    fn drop(&mut self) {
        #[cfg(feature = "allocation-observation")]
        ALLOCATION_WINDOW_ACTIVE.store(false, Ordering::Release);
    }
}

#[cfg(all(test, feature = "allocation-observation"))]
mod tests {
    #[test]
    fn failed_work_disables_its_allocation_window() {
        use super::*;

        let failed = || -> Result<(), ()> {
            let _window = AllocationWindow::begin();
            assert!(ALLOCATION_WINDOW_ACTIVE.load(Ordering::Acquire));
            Err(())
        };
        assert!(failed().is_err());
        assert!(!ALLOCATION_WINDOW_ACTIVE.load(Ordering::Acquire));
        let window = AllocationWindow::begin();
        let _totals = window.finish();
        assert!(!ALLOCATION_WINDOW_ACTIVE.load(Ordering::Acquire));
    }
}
