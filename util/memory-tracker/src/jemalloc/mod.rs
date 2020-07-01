mod profiling;
mod report;

#[cfg(any(target_env = "msvc", target_os = "macos", feature = "disable-jemalloc"))]
mod mocked_statistics;
#[cfg(all(
    not(target_env = "msvc"),
    not(target_os = "macos"),
    not(feature = "disable-jemalloc")
))]
mod statistics;

pub use profiling::jemalloc_profiling_dump;
pub use report::{JeMallocMemoryReport, JeMallocReportHandle};
