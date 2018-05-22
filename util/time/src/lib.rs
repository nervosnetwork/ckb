pub use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
}

pub fn now_ms() -> u64 {
    let duration = now();
    duration.as_secs() * 1000 + u64::from(duration.subsec_nanos()) / 1_000_000
}
