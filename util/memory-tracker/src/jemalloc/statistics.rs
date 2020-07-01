use std::fmt;

use ckb_logger::error;
use jemalloc_ctl::{epoch, stats};

use crate::utils::PropertyValue;

macro_rules! init_mib {
    ($key:ty) => {
        <$key>::mib()
            .map_err(|err| {
                error!(
                    "failed to lookup jemalloc mib for {}: {}",
                    stringify!($key),
                    err
                );
            })
            .ok()
    };
}

macro_rules! mib_read {
    ($mib:expr) => {
        match $mib.read() {
            Ok(value) => PropertyValue::Value(value as u64),
            Err(err) => {
                let error = format!(
                    "failed to read jemalloc stats for {}: {}",
                    stringify!($mib),
                    err
                );
                PropertyValue::Error(error)
            }
        }
    };
}

// MIB: Management Information Base
pub(super) struct JeMallocMIBs {
    epoch: jemalloc_ctl::epoch_mib,
    allocated: stats::allocated_mib,
    resident: stats::resident_mib,
    active: stats::active_mib,
    mapped: stats::mapped_mib,
    retained: stats::retained_mib,
    metadata: stats::metadata_mib,
}

pub(super) struct JeMallocMemoryStatistics {
    // Bytes allocated by the application.
    allocated: PropertyValue<u64>,
    // Bytes in physically resident data pages mapped by the allocator.
    resident: PropertyValue<u64>,
    // Bytes in active pages allocated by the application.
    active: PropertyValue<u64>,
    // Bytes in active extents mapped by the allocator.
    mapped: PropertyValue<u64>,
    // Bytes in virtual memory mappings that were retained
    // rather than being returned to the operating system
    retained: PropertyValue<u64>,
    // Bytes dedicated to jemalloc metadata.
    metadata: PropertyValue<u64>,
}

impl fmt::Display for JeMallocMemoryStatistics {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("JeMalloc")
            .field("allocated", &self.allocated)
            .field("resident", &self.resident)
            .field("active", &self.active)
            .field("mapped", &self.mapped)
            .field("retained", &self.retained)
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl JeMallocMIBs {
    pub(super) fn initialize() -> Option<Self> {
        Some(Self {
            epoch: init_mib!(epoch)?,
            allocated: init_mib!(stats::allocated)?,
            resident: init_mib!(stats::resident)?,
            active: init_mib!(stats::active)?,
            mapped: init_mib!(stats::mapped)?,
            retained: init_mib!(stats::retained)?,
            metadata: init_mib!(stats::metadata)?,
        })
    }

    pub(super) fn stats(&self) -> Option<JeMallocMemoryStatistics> {
        self.epoch
            .advance()
            .map(|_| JeMallocMemoryStatistics {
                allocated: mib_read!(self.allocated),
                resident: mib_read!(self.resident),
                active: mib_read!(self.active),
                mapped: mib_read!(self.mapped),
                retained: mib_read!(self.retained),
                metadata: mib_read!(self.metadata),
            })
            .map_err(|err| {
                error!("failed to refresh the jemalloc stats: {}", err);
            })
            .ok()
    }
}
