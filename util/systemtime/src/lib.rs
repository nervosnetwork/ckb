//! ckb_systemtime provide real system timestamp, and faketime when `enable_faketime` feature is enabled.
mod test_faketime;
mod test_realtime;

#[cfg(feature = "enable_faketime")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(feature = "enable_faketime")]
use std::sync::{Mutex, MutexGuard};
#[cfg(not(target_family = "wasm"))]
pub use std::time::{Duration, Instant, SystemTime};
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub use web_time::{Duration, Instant, SystemTime};

// Store faketime timestamp here
#[cfg(feature = "enable_faketime")]
static FAKETIME: AtomicU64 = AtomicU64::new(0);

// Indicate whether faketime is enabled
#[cfg(feature = "enable_faketime")]
static FAKETIME_ENABLED: AtomicBool = AtomicBool::new(false);

// Mutex to serialise faketime access across parallel tests.
// Without this, one test's FaketimeGuard::drop() can disable faketime
// while another test is still relying on it.
#[cfg(feature = "enable_faketime")]
static FAKETIME_MUTEX: Mutex<()> = Mutex::new(());

// Get real system's timestamp in millis
fn system_time_as_millis() -> u64 {
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("SystemTime before UNIX EPOCH!");
    duration.as_secs() * 1000 + u64::from(duration.subsec_millis())
}

/// Get system's timestamp in millis
#[cfg(not(feature = "enable_faketime"))]
pub fn unix_time_as_millis() -> u64 {
    system_time_as_millis()
}

/// Return FaketimeGuard to set/disable faketime.
///
/// The returned guard holds a process-wide mutex so that concurrent tests
/// cannot interfere with each other's faketime settings.
#[cfg(feature = "enable_faketime")]
pub fn faketime() -> FaketimeGuard {
    let lock = FAKETIME_MUTEX.lock().expect("FAKETIME_MUTEX poisoned");
    FaketimeGuard { _lock: lock }
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
/// and will disable faketime when dropped.
///
/// It holds a mutex guard to ensure only one test can manipulate
/// faketime at a time, preventing race conditions in parallel test execution.
#[cfg(feature = "enable_faketime")]
pub struct FaketimeGuard {
    _lock: MutexGuard<'static, ()>,
}

#[cfg(feature = "enable_faketime")]
impl FaketimeGuard {
    /// Set faketime
    #[cfg(feature = "enable_faketime")]
    pub fn set_faketime(&self, time: u64) {
        FAKETIME.store(time, Ordering::Release);
        FAKETIME_ENABLED.store(true, Ordering::SeqCst);
    }

    /// Disable faketime
    #[cfg(feature = "enable_faketime")]
    pub fn disable_faketime(&self) {
        FAKETIME_ENABLED.store(false, Ordering::Release);
    }
}

#[cfg(feature = "enable_faketime")]
impl Drop for FaketimeGuard {
    fn drop(&mut self) {
        self.disable_faketime()
    }
}