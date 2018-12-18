use std::time::Duration;

pub trait DurationExt {
    /// Returns the total number of whole milliseconds contained by this `Duration` as `u64`.
    ///
    /// # Panics
    ///
    /// Panics if the duration is overflow as `u64`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use ckb_time::prelude::*;
    ///
    /// let duration = Duration::new(5, 730023852);
    /// assert_eq!(duration.as_millis_u64(), 5730);
    /// ```
    fn as_millis_u64(&self) -> u64;
}

impl DurationExt for Duration {
    fn as_millis_u64(&self) -> u64 {
        self.as_secs() * 1000 + u64::from(self.subsec_millis())
    }
}
