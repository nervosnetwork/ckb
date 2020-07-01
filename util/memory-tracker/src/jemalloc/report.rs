use std::fmt;

#[cfg(all(
    not(target_env = "msvc"),
    not(target_os = "macos"),
    not(feature = "disable-jemalloc")
))]
use super::statistics::{JeMallocMIBs, JeMallocMemoryStatistics};

#[cfg(any(target_env = "msvc", target_os = "macos", feature = "disable-jemalloc"))]
use super::mocked_statistics::{JeMallocMIBs, JeMallocMemoryStatistics};

pub struct JeMallocReportHandle {
    mibs: Option<JeMallocMIBs>,
}

pub struct JeMallocMemoryReport {
    stats: Option<JeMallocMemoryStatistics>,
}

impl fmt::Display for JeMallocMemoryReport {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(ref stats) = self.stats {
            write!(f, "{}", stats)
        } else {
            write!(f, "JeMalloc {{ null }}")
        }
    }
}

impl JeMallocReportHandle {
    pub fn initialize() -> Self {
        Self {
            mibs: JeMallocMIBs::initialize(),
        }
    }

    pub fn report(&self) -> JeMallocMemoryReport {
        self.mibs
            .as_ref()
            .and_then(JeMallocMIBs::stats)
            .map(JeMallocMemoryReport::new)
            .unwrap_or_else(JeMallocMemoryReport::null)
    }
}

impl JeMallocMemoryReport {
    fn new(stats: JeMallocMemoryStatistics) -> Self {
        Self { stats: Some(stats) }
    }

    fn null() -> Self {
        Self { stats: None }
    }
}
