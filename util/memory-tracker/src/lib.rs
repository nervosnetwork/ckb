mod config;

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
    use ckb_logger::warn;

    pub fn jemalloc_profiling_dump(_: String) {
        warn!("jemalloc profiling dump: unsupported");
    }
}

#[cfg(all(not(target_env = "msvc"), not(target_os = "macos")))]
mod process;
#[cfg(not(all(not(target_env = "msvc"), not(target_os = "macos"))))]
mod process {
    use ckb_logger::info;

    pub fn track_current_process(_: u64) {
        info!("track current process: unsupported");
    }
}

pub use config::Config;
pub use jemalloc::jemalloc_profiling_dump;
pub use process::track_current_process;
