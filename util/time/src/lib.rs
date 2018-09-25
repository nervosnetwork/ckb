use std::cell::Cell;
pub use std::time::Duration;
#[cfg(not(ckb_test))]
use std::time::{SystemTime, UNIX_EPOCH};

thread_local! {
    pub static MOCK_TIMER: Cell<Duration> = Cell::new(Duration::from_millis(0));
}

#[cfg(ckb_test)]
pub fn now() -> Duration {
    MOCK_TIMER.with(|t| t.get())
}

#[cfg(ckb_test)]
pub fn set_mock_timer(ms: u64) {
    MOCK_TIMER.with(|t| {
        t.replace(Duration::from_millis(ms));
    });
}

#[cfg(not(ckb_test))]
pub fn now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
}

pub fn now_ms() -> u64 {
    let duration = now();
    duration.as_secs() * 1000 + u64::from(duration.subsec_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_timer() {
        assert_eq!(now_ms(), 0);
        set_mock_timer(100);
        assert_eq!(now_ms(), 100);
    }
}
