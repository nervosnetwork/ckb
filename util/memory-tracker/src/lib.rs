//! Track the memory usage of CKB.

#[cfg(all(
    not(target_env = "msvc"),
    not(target_os = "macos"),
    feature = "profiling"
))]
mod jemalloc;
#[cfg(not(all(
    not(target_env = "msvc"),
    not(target_os = "macos"),
    feature = "profiling"
)))]
mod jemalloc {
    /// A dummy function which is used when the Jemalloc profiling isn't supported.
    ///
    /// Jemalloc profiling is disabled in default, the feature `profiling` is used to enable it.
    pub fn jemalloc_profiling_dump(_: &str) -> Result<(), String> {
        Err("jemalloc profiling dump: unsupported".to_string())
    }
}

#[cfg(all(not(target_env = "msvc"), not(target_os = "macos")))]
mod process;
#[cfg(not(all(not(target_env = "msvc"), not(target_os = "macos"))))]
mod process {
    use std::sync;

    use crate::rocksdb::TrackRocksDBMemory;
    use ckb_logger::info;

    /// A dummy function which is used when tracking memory usage isn't supported.
    pub fn track_current_process<Tracker: 'static + TrackRocksDBMemory + Sync + Send>(
        _: u64,
        _: Option<sync::Arc<Tracker>>,
    ) {
        info!("track current process: unsupported");
    }
}
mod rocksdb;

pub use jemalloc::jemalloc_profiling_dump;
pub use process::track_current_process;
pub use rocksdb::TrackRocksDBMemory;

/// Track the memory usage of the CKB process and Jemalloc.
pub fn track_current_process_simple(interval: u64) {
    track_current_process::<rocksdb::DummyRocksDB>(interval, None);
}
