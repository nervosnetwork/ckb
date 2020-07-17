use std::{sync, thread, time};

use ckb_logger::{debug, error, info};
use futures::executor::block_on;
use heim::units::information::byte;
use jemalloc_ctl::{epoch, stats};

use crate::{rocksdb::TrackRocksDBMemory, utils::HumanReadableSize};

macro_rules! je_mib {
    ($key:ty) => {
        if let Ok(value) = <$key>::mib() {
            value
        } else {
            error!("failed to lookup jemalloc mib for {}", stringify!($key));
            return;
        }
    };
}

macro_rules! mib_read {
    ($mib:ident) => {
        if let Ok(value) = $mib.read() {
            HumanReadableSize::from(value as u64)
        } else {
            error!("failed to read jemalloc stats for {}", stringify!($mib));
            return;
        }
    };
}

pub fn track_current_process<Tracker: 'static + TrackRocksDBMemory + Sync + Send>(
    interval: u64,
    tracker_opt: Option<sync::Arc<Tracker>>,
) {
    if interval == 0 {
        info!("track current process: disable");
    } else {
        info!("track current process: enable");
        let wait_secs = time::Duration::from_secs(interval);

        let je_epoch = je_mib!(epoch);
        // Bytes allocated by the application.
        let allocated = je_mib!(stats::allocated);
        // Bytes in physically resident data pages mapped by the allocator.
        let resident = je_mib!(stats::resident);
        // Bytes in active pages allocated by the application.
        let active = je_mib!(stats::active);
        // Bytes in active extents mapped by the allocator.
        let mapped = je_mib!(stats::mapped);
        // Bytes in virtual memory mappings that were retained
        // rather than being returned to the operating system
        let retained = je_mib!(stats::retained);
        // Bytes dedicated to jemalloc metadata.
        let metadata = je_mib!(stats::metadata);

        if let Err(err) = thread::Builder::new()
            .name("MemoryTracker".to_string())
            .spawn(move || {
                if let Ok(process) = block_on(heim::process::current()) {
                    let pid = process.pid();
                    loop {
                        if je_epoch.advance().is_err() {
                            error!("failed to refresh the jemalloc stats");
                            return;
                        }
                        if let Ok(memory) = block_on(process.memory()) {
                            // Resident set size, amount of non-swapped physical memory.
                            let rss: HumanReadableSize = memory.rss().get::<byte>().into();
                            // Virtual memory size, total amount of memory.
                            let virt: HumanReadableSize = memory.vms().get::<byte>().into();

                            let allocated = mib_read!(allocated);
                            let resident = mib_read!(resident);
                            let active = mib_read!(active);
                            let mapped = mib_read!(mapped);
                            let retained = mib_read!(retained);
                            let metadata = mib_read!(metadata);

                            if let Some(tracker) = tracker_opt.clone() {
                                let stats = tracker.gather_memory_stats();
                                debug!(
                                    "CurrentProcess {{ \
                                        pid: {}, rss: {}, virt: {}, \
                                        Jemalloc: {{ \
                                            allocated: {}, resident: {}, \
                                            active: {}, mapped: {}, retained: {}, \
                                            metadata: {} }}, \
                                        RocksDB: {{ \
                                            total: {}, cache: {}, readers: {}, \
                                            memtables: {}, pinned: {}, \
                                            cache-capacity: {} \
                                        }} \
                                    }}",
                                    pid,
                                    rss,
                                    virt,
                                    allocated,
                                    resident,
                                    active,
                                    mapped,
                                    retained,
                                    metadata,
                                    stats.total_memory,
                                    stats.block_cache_usage,
                                    stats.estimate_table_readers_mem,
                                    stats.cur_size_all_mem_tables,
                                    stats.block_cache_pinned_usage,
                                    stats.block_cache_capacity,
                                );
                            } else {
                                debug!(
                                    "CurrentProcess {{ \
                                        pid: {}, rss: {}, virt: {}, \
                                        Jemalloc: {{ \
                                            allocated: {}, resident: {}, \
                                            active: {}, mapped: {}, retained: {}, \
                                            metadata: {} }} \
                                    }}",
                                    pid,
                                    rss,
                                    virt,
                                    allocated,
                                    resident,
                                    active,
                                    mapped,
                                    retained,
                                    metadata,
                                );
                            }
                        } else {
                            error!("failed to fetch the memory information about current process");
                        }
                        thread::sleep(wait_secs);
                    }
                } else {
                    error!("failed to track the currently running program");
                }
            })
        {
            error!(
                "failed to spawn the thread to track current process: {}",
                err
            );
        }
    }
}
