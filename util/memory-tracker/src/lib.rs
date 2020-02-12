mod config;
#[cfg_attr(
    all(not(target_env = "msvc"), not(target_os = "macos")),
    path = "jemalloc.rs"
)]
#[cfg_attr(
    not(all(not(target_env = "msvc"), not(target_os = "macos"))),
    path = "jemalloc-mock.rs"
)]
mod jemalloc;
#[cfg_attr(
    all(not(target_env = "msvc"), not(target_os = "macos")),
    path = "process.rs"
)]
#[cfg_attr(
    not(all(not(target_env = "msvc"), not(target_os = "macos"))),
    path = "process-mock.rs"
)]
mod process;

pub use config::Config;
pub use jemalloc::jemalloc_profiling_dump;
pub use process::track_current_process;
