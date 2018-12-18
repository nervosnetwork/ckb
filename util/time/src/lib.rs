pub mod system;

mod ext;

#[cfg(not(feature = "mock_timer"))]
pub use crate::system::now;

#[cfg(feature = "mock_timer")]
mod mock;
#[cfg(feature = "mock_timer")]
pub use crate::mock::now;
#[doc(hidden)]
#[cfg(feature = "mock_timer")]
pub use crate::mock::{mock_time, TimeMock};

pub use crate::ext::DurationExt;

pub fn now_ms() -> u64 {
    now().as_millis_u64()
}

#[cfg(feature = "mock_timer")]
pub fn set_mock_timer(ms: u64) {
    mock_time(TimeMock::constant(ms as i64)).persist();
}

pub mod prelude {
    pub use super::DurationExt;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "mock_timer"))]
    #[test]
    fn test_system_time() {
        let system_now_ms = system::now().as_millis_u64();
        assert!(now().as_millis_u64() - system_now_ms < 60000);
    }

    #[cfg(feature = "mock_timer")]
    #[test]
    fn test_mock_constant_time() {
        let _time_guard = mock_time(TimeMock::constant(100));
        assert_eq!(now().as_millis_u64(), 100);
    }
}
