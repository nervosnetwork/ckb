use std::{sync, thread, time};

use ckb_logger::{error, info};
use ckb_metrics::metrics;
use futures::executor::block_on;
use heim::units::information::byte;
use jemalloc_ctl::{epoch, stats};

use crate::rocksdb::TrackRocksDBMemory;

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
            value as i64
        } else {
            error!("failed to read jemalloc stats for {}", stringify!($mib));
            return;
        }
    };
}

/// Track the memory usage of the CKB process, Jemalloc and RocksDB through [ckb-metrics](../../ckb_metrics/index.html).
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
                    loop {
                        if je_epoch.advance().is_err() {
                            error!("failed to refresh the jemalloc stats");
                            return;
                        }
                        if let Ok(memory) = block_on(process.memory()) {
                            // Resident set size, amount of non-swapped physical memory.
                            let rss = memory.rss().get::<byte>() as i64;
                            // Virtual memory size, total amount of memory.
                            let vms = memory.vms().get::<byte>() as i64;

                            metrics!(gauge, "ckb-sys.mem.process", rss, "type" => "rss");
                            metrics!(gauge, "ckb-sys.mem.process", vms, "type" => "vms");

                            let allocated = mib_read!(allocated);
                            let resident = mib_read!(resident);
                            let active = mib_read!(active);
                            let mapped = mib_read!(mapped);
                            let retained = mib_read!(retained);
                            let metadata = mib_read!(metadata);

                            metrics!(gauge, "ckb-sys.mem.jemalloc", allocated, "type" => "allocated");
                            metrics!(gauge, "ckb-sys.mem.jemalloc", resident, "type" => "resident");
                            metrics!(gauge, "ckb-sys.mem.jemalloc", active, "type" => "active");
                            metrics!(gauge, "ckb-sys.mem.jemalloc", mapped, "type" => "mapped");
                            metrics!(gauge, "ckb-sys.mem.jemalloc", retained, "type" => "retained");
                            metrics!(gauge, "ckb-sys.mem.jemalloc", metadata, "type" => "metadata");

                            if let Some(tracker) = tracker_opt.clone() {
                                tracker.gather_memory_stats();
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
