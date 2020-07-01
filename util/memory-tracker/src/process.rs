use std::fmt;

use ckb_logger::error;
use futures::executor::block_on;
use heim::process::{Pid, Process};
use heim::units::information::byte;

use crate::utils::{PropertyValue, Size};

pub struct ProcessTracker {
    process: Option<Process>,
}

struct ProcessStatistics {
    pid: Pid,
    // Resident set size, amount of non-swapped physical memory.
    rss: PropertyValue<Size>,
    // Virtual memory size, total amount of memory.
    virt: PropertyValue<Size>,
}

pub struct ProcessReport {
    stats: Option<ProcessStatistics>,
}

impl fmt::Display for ProcessStatistics {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Process")
            .field("pid", &self.pid)
            .field("rss", &self.rss)
            .field("virt", &self.virt)
            .finish()
    }
}

impl fmt::Display for ProcessReport {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(ref stats) = self.stats {
            write!(f, "{}", stats)
        } else {
            write!(f, "Process {{ null }}")
        }
    }
}

impl ProcessTracker {
    pub(super) fn initialize() -> Self {
        let process = block_on(heim::process::current())
            .map_err(|err| {
                error!("failed to track the currently running program: {}", err);
            })
            .ok();
        Self { process }
    }

    pub(super) fn report(&self) -> ProcessReport {
        self.process
            .as_ref()
            .map(|ref process| {
                let pid = process.pid();
                let (rss, virt) = block_on(process.memory())
                    .map(|memory| {
                        let rss: Size = memory.rss().get::<byte>().into();
                        let virt: Size = memory.vms().get::<byte>().into();
                        (PropertyValue::new(rss), PropertyValue::new(virt))
                    })
                    .unwrap_or_else(|err| {
                        error!(
                            "failed to fetch the memory information about current process: {}",
                            err
                        );
                        Default::default()
                    });
                ProcessStatistics { pid, rss, virt }
            })
            .map(ProcessReport::new)
            .unwrap_or_else(ProcessReport::null)
    }
}

impl ProcessReport {
    fn new(stats: ProcessStatistics) -> Self {
        Self { stats: Some(stats) }
    }

    fn null() -> Self {
        Self { stats: None }
    }
}
