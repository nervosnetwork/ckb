use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Gets current time as `Duration` since unix epoch.
pub fn now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("SystemTime before UNIX EPOCH!")
}
