use std::{fmt, sync, thread, time};

use ckb_logger::{debug, error, info, trace};
use crossbeam_channel::{select, unbounded};

use crate::{
    collections,
    jemalloc::{JeMallocMemoryReport, JeMallocReportHandle},
    process::{ProcessReport, ProcessTracker},
    rocksdb::{RocksDBMemoryStatistics, TrackRocksDBMemory},
};

struct ComprehensiveReport {
    process: ProcessReport,
    jemalloc: JeMallocMemoryReport,
    rocksdb: Option<RocksDBMemoryStatistics>,
}

impl fmt::Display for ComprehensiveReport {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{{")?;
        write!(f, " process: {}", self.process)?;
        write!(f, ", allocator: {}", self.jemalloc)?;
        if let Some(ref stats) = self.rocksdb {
            write!(f, ", database: {}", stats)?;
        }
        write!(f, " }}")
    }
}

pub fn track_current_process<DBTracker: 'static + TrackRocksDBMemory + Sync + Send>(
    interval: u64,
    db_tracker_opt: Option<sync::Arc<DBTracker>>,
) {
    if interval == 0 {
        info!("track current process: disable");
        return;
    }

    info!(
        "track current process: enable (interval: {} seconds)",
        interval
    );
    crate::set_interval(interval);
    let wait_secs = time::Duration::from_secs(interval);

    let (sender, receiver) = unbounded();
    collections::MEASURE_SENDER.write().replace(sender);

    let jemalloc_handle = JeMallocReportHandle::initialize();

    let thread_res = thread::Builder::new()
        .name("MemoryTracker".to_string())
        .spawn(move || {
            trace!("MemoryTracker is running ...");

            let process_tracker = ProcessTracker::initialize();

            let mut now = time::Instant::now();
            loop {
                if now.elapsed().as_secs() >= interval {
                    now = time::Instant::now();

                    let jemalloc_report = jemalloc_handle.report();
                    let process_report = process_tracker.report();
                    let rocksdb_report_opt =
                        db_tracker_opt.as_ref().map(|t| t.gather_memory_stats());

                    let full_report = ComprehensiveReport {
                        jemalloc: jemalloc_report,
                        process: process_report,
                        rocksdb: rocksdb_report_opt,
                    };

                    debug!("{}", full_report);

                    collections::track_collections();
                }
                select! {
                    recv(receiver) -> item => {
                        if let Ok((tag, record)) = item {
                            collections::STATISTICS.write().insert(tag, record);
                        }
                    }
                    default(wait_secs) => {
                    }
                }
            }
        });

    if let Err(err) = thread_res {
        error!(
            "failed to spawn the thread to track current process: {}",
            err
        );
    }
}
