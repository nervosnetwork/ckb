use std::fmt;

pub(super) struct JeMallocMIBs;

pub(super) struct JeMallocMemoryStatistics;

impl fmt::Display for JeMallocMemoryStatistics {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "JeMalloc: disabled")
    }
}

impl JeMallocMIBs {
    pub(super) fn initialize() -> Option<Self> {
        None
    }

    pub(super) fn stats(&self) -> Option<JeMallocMemoryStatistics> {
        None
    }
}
