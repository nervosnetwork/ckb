mod config;
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
pub use process::track_current_process;
