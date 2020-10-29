//! TODO(doc): @yangby-cryptape
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
    /// TODO(doc): @yangby-cryptape
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

    /// TODO(doc): @yangby-cryptape
    pub fn track_current_process<Tracker: 'static + TrackRocksDBMemory + Sync + Send>(
        _: u64,
        _: Option<sync::Arc<Tracker>>,
    ) {
        info!("track current process: unsupported");
    }
}
pub mod rocksdb;
pub mod utils;

pub use jemalloc::jemalloc_profiling_dump;
pub use process::track_current_process;

/// TODO(doc): @yangby-cryptape
pub fn track_current_process_simple(interval: u64) {
    track_current_process::<rocksdb::DummyRocksDB>(interval, None);
}
