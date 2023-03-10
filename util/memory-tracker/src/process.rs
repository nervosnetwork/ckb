use std::{fs, io, str::FromStr, sync, thread, time};

use ckb_logger::{error, info};
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
                loop {
                    if je_epoch.advance().is_err() {
                        error!("failed to refresh the jemalloc stats");
                        return;
                    }
                    if let Ok(memory) = get_current_process_memory() {
                        // Resident set size, amount of non-swapped physical memory.
                        let rss = memory.resident as i64;
                        // Virtual memory size, total amount of memory.
                        let vms = memory.size as i64;

                        if let Some(metrics) = ckb_metrics::handle() {
                            metrics.ckb_sys_mem_process.rss.set(rss);
                            metrics.ckb_sys_mem_process.vms.set(vms);
                        }

                        let allocated = mib_read!(allocated);
                        let resident = mib_read!(resident);
                        let active = mib_read!(active);
                        let mapped = mib_read!(mapped);
                        let retained = mib_read!(retained);
                        let metadata = mib_read!(metadata);
                        if let Some(metrics) = ckb_metrics::handle() {
                            metrics.ckb_sys_mem_jemalloc.allocated.set(allocated);
                            metrics.ckb_sys_mem_jemalloc.resident.set(resident);
                            metrics.ckb_sys_mem_jemalloc.active.set(active);
                            metrics.ckb_sys_mem_jemalloc.mapped.set(mapped);
                            metrics.ckb_sys_mem_jemalloc.retained.set(retained);
                            metrics.ckb_sys_mem_jemalloc.metadata.set(metadata);

                            if let Some(tracker) = tracker_opt.clone() {
                                tracker.gather_memory_stats();
                            }
                        }
                    } else {
                        error!("failed to fetch the memory information about current process");
                    }
                    thread::sleep(wait_secs);
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

#[derive(Debug)]
pub struct Memory {
    // Virtual memory size
    size: u64,
    // Size of physical memory being used
    resident: u64,
    // Number of shared pages
    _shared: u64,
    // The size of executable virtual memory owned by the program
    _text: u64,
    // Size of the program data segment and the user state stack
    _data: u64,
}

impl FromStr for Memory {
    type Err = io::Error;
    fn from_str(value: &str) -> Result<Memory, io::Error> {
        static PAGE_SIZE: once_cell::sync::OnceCell<u64> = once_cell::sync::OnceCell::new();
        let page_size =
            PAGE_SIZE.get_or_init(|| unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 });
        let mut parts = value.split_ascii_whitespace();
        let size = parts
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
            .and_then(|value| {
                u64::from_str(value)
                    .map(|value| value * *page_size)
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
            })?;
        let resident = parts
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
            .and_then(|value| {
                u64::from_str(value)
                    .map(|value| value * *page_size)
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
            })?;
        let _shared = parts
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
            .and_then(|value| {
                u64::from_str(value)
                    .map(|value| value * *page_size)
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
            })?;
        let _text = parts
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
            .and_then(|value| {
                u64::from_str(value)
                    .map(|value| value * *page_size)
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
            })?;
        // ignore the size of the library in the virtual memory space of the task being imaged
        let _lrs = parts.next();
        let _data = parts
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
            .and_then(|value| {
                u64::from_str(value)
                    .map(|value| value * *page_size)
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
            })?;
        Ok(Memory {
            size,
            resident,
            _shared,
            _text,
            _data,
        })
    }
}

fn get_current_process_memory() -> Result<Memory, io::Error> {
    static PID: once_cell::sync::OnceCell<libc::pid_t> = once_cell::sync::OnceCell::new();
    let pid = PID.get_or_init(|| unsafe { libc::getpid() });
    let content = fs::read_to_string(format!("/proc/{pid}/statm"))?;

    Memory::from_str(&content)
}
