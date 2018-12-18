use crate::ext::DurationExt;
use crate::system::now as system_now;

use std::cell::Cell;
use std::time::Duration;

/// Mock time using 1D transform.
///
/// ```
/// # let x = 50u64;
/// # let scale = 1f64;
/// # let translate = 100i64;
/// use ckb_time::TimeMock;
/// let t = TimeMock::new(scale, translate);
/// assert_eq!(((scale * x as f64) as i64 + translate) as u64, t.apply(x));
/// ```
#[derive(Copy, Clone, Debug)]
pub struct TimeMock {
    scale: f64,
    translate: i64,
}

impl TimeMock {
    pub fn new(scale: f64, translate: i64) -> TimeMock {
        TimeMock { scale, translate }
    }

    pub fn with_scale(scale: f64) -> TimeMock {
        TimeMock {
            translate: 0,
            scale,
        }
    }

    pub fn with_translate(translate: i64) -> TimeMock {
        TimeMock {
            scale: 1.0,
            translate,
        }
    }

    pub fn constant(translate: i64) -> TimeMock {
        TimeMock {
            scale: 0.0,
            translate,
        }
    }

    pub fn scale(self, scale: f64) -> TimeMock {
        TimeMock {
            scale: self.scale * scale,
            translate: (self.translate as f64 * scale) as i64,
        }
    }

    pub fn translate(self, translate: i64) -> TimeMock {
        TimeMock {
            scale: self.scale,
            translate: self.translate + translate,
        }
    }

    pub fn apply(&self, x: u64) -> u64 {
        ((self.scale * x as f64) as i64 + self.translate) as u64
    }
}

impl Default for TimeMock {
    fn default() -> TimeMock {
        TimeMock::new(1.0, 0)
    }
}

thread_local! {
    // TODO: (ian) I want to use the system time by default, and only opt in the mock time after
    // using `mock_time`. But it will fail the integrationt tests in sync/src/tests/.
    pub static MOCK_TIME: Cell<Option<TimeMock>> = Cell::new(Some(TimeMock::constant(0)));
}

#[must_use = "this value should be used"]
pub struct MockGuard {
    restore_to: Option<TimeMock>,
    should_restore: bool,
}

impl MockGuard {
    /// Make the transform permanent.
    pub fn persist(&mut self) {
        self.should_restore = false;
    }
}

impl Drop for MockGuard {
    fn drop(&mut self) {
        if self.should_restore {
            MOCK_TIME.with(|t| t.replace(self.restore_to));
        }
    }
}

/// Gets current time as `Duration` since unix epoch.
pub fn now() -> Duration {
    MOCK_TIME.with(|t| {
        t.get().map_or_else(system_now, |trans| {
            Duration::from_millis(trans.apply(system_now().as_millis_u64()))
        })
    })
}

pub fn mock_time(transform: TimeMock) -> MockGuard {
    MOCK_TIME.with(|t| MockGuard {
        restore_to: t.replace(Some(transform)),
        should_restore: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_with_scale() {
        assert_eq!(TimeMock::with_scale(2.0).apply(4), 8);
        assert_eq!(TimeMock::with_scale(0.5).apply(4), 2);
    }

    #[test]
    fn test_with_translate() {
        assert_eq!(TimeMock::with_translate(2).apply(10), 12);
        assert_eq!(TimeMock::with_translate(-2).apply(10), 8);
    }

    #[test]
    fn test_scale() {
        let old = TimeMock::new(2.0, 4);
        let t = old.clone().scale(2.0);

        assert_eq!(2 * old.apply(10), t.apply(10))
    }

    #[test]
    fn test_translate() {
        let old = TimeMock::new(2.0, 4);
        let t = old.clone().translate(5);

        assert_eq!(5 + old.apply(10), t.apply(10))
    }

    #[test]
    fn test_constant() {
        assert_eq!(TimeMock::constant(2).apply(4), 2);
        assert_eq!(TimeMock::constant(2).apply(2), 2);
        assert_eq!(TimeMock::constant(2).apply(1), 2);
    }

    #[test]
    fn test_nested_mock_scope() {
        {
            let _time_guard = mock_time(TimeMock::constant(100));
            assert_eq!(now().as_millis_u64(), 100);
            {
                let _time_guard = mock_time(TimeMock::constant(50));
                assert_eq!(now().as_millis_u64(), 50);
            }
            assert_eq!(now().as_millis_u64(), 100);
        }
        // let system_now_ms = system_now().as_millis_u64();
        // assert!(now().as_millis_u64() - system_now_ms < 60000);
        assert_eq!(0, now().as_millis_u64());
    }

    #[test]
    fn test_persist_mock() {
        {
            mock_time(TimeMock::constant(100)).persist();
            assert_eq!(now().as_millis_u64(), 100);
        }
        assert_eq!(now().as_millis_u64(), 100);
        mock_time(TimeMock::constant(0)).persist();
        assert_eq!(0, now().as_millis_u64());
    }
}
