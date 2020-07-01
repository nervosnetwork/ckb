use ckb_util::RwLock;
use std::sync::Arc;

lazy_static::lazy_static! {
    static ref INTERVAL: Arc<RwLock<u64>> = Arc::new(RwLock::new(0));
}

pub mod collections;
pub(crate) mod jemalloc;
pub(crate) mod process;
pub mod rocksdb;
pub mod utils;

mod service;

pub use jemalloc::jemalloc_profiling_dump;
pub use service::track_current_process;

pub fn interval() -> u64 {
    *INTERVAL.read()
}

pub(crate) fn set_interval(interval: u64) {
    *crate::INTERVAL.write() = interval;
}

pub fn track_current_process_simple(interval: u64) {
    track_current_process::<rocksdb::DummyRocksDB>(interval, None);
}
