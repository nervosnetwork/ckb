use std::time::{SystemTime, UNIX_EPOCH};
pub use std::time::Duration;

pub fn now_ms() -> u64 {
    let start = SystemTime::now();
    let since_the_epoch = start
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");

    since_the_epoch.as_secs() * 1000 + u64::from(since_the_epoch.subsec_nanos()) / 1_000_000
}
