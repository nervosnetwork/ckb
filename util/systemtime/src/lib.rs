//! ckb_systemtime provide real system timestamp, and faketime when `enable_faketime` feature is enabled.
mod test_faketime;
mod test_realtime;

#[cfg(feature = "enable_faketime")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

// Store faketime timestamp here
#[cfg(feature = "enable_faketime")]
static FAKETIME: AtomicU64 = AtomicU64::new(0);

// Indicate whether faketime is enabled
#[cfg(feature = "enable_faketime")]
static FAKETIME_ENABLED: AtomicBool = AtomicBool::new(false);

// Get real system's timestamp in millis
fn system_time_as_millis() -> u64 {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("SystemTime before UNIX EPOCH!");
    duration.as_secs() * 1000 + u64::from(duration.subsec_millis())
}

/// Get system's timestamp in millis
#[cfg(not(feature = "enable_faketime"))]
pub fn unix_time_as_millis() -> u64 {
    system_time_as_millis()
}

/// Return FaketimeGuard to set/disable faketime
#[cfg(feature = "enable_faketime")]
pub fn faketime() -> FaketimeGuard {
    FaketimeGuard {}
}

/// Get fake timestamp in millis, only available when `enable_faketime` feature is enabled
#[cfg(feature = "enable_faketime")]
pub fn unix_time_as_millis() -> u64 {
    if FAKETIME_ENABLED.load(Ordering::SeqCst) {
        return FAKETIME.load(Ordering::SeqCst);
    }
    system_time_as_millis()
}

/// Get system's unix_time
pub fn unix_time() -> Duration {
    Duration::from_millis(unix_time_as_millis())
}

/// FaketimeGuard is used to set/disable faketime,
/// and will disable faketime when dropped
#[cfg(feature = "enable_faketime")]
pub struct FaketimeGuard {}

#[cfg(feature = "enable_faketime")]
impl FaketimeGuard {
    /// Set faketime
    #[cfg(feature = "enable_faketime")]
    pub fn set_faketime(&self, time: u64) {
        FAKETIME.store(time, Ordering::Relaxed);
        FAKETIME_ENABLED.store(true, Ordering::SeqCst);
    }

    /// Disable faketime
    #[cfg(feature = "enable_faketime")]
    pub fn disable_faketime(&self) {
        FAKETIME_ENABLED.store(false, Ordering::Relaxed);
    }
}

#[cfg(feature = "enable_faketime")]
impl Drop for FaketimeGuard {
    fn drop(&mut self) {
        self.disable_faketime()
    }
}
