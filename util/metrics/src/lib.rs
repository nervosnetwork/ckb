//! A lightweight metrics facade used in CKB.
//!
//! The `ckb-metrics` crate is a wrapper of [`metrics`]. The crate [`ckb-metrics-service`] is the
//! runtime which handles the metrics data in CKB.
//!
//! [`metrics`]: https://docs.rs/metrics/*/metrics/index.html
//! [`ckb-metrics-service`]: ../ckb_metrics_service/index.html
//!
//! ## Use
//!
//! The basic use of the facade crate is through the metrics macro: [`metrics!`].
//!
//! ### Examples
//!
//! ```rust
//! use ckb_metrics::metrics;
//!
//! # use std::time::Instant;
//! # pub fn run_query(_: &str) -> u64 { 42 }
//! pub fn process(query: &str) -> u64 {
//!     let start = Instant::now();
//!     let row_count = run_query(query);
//!     let end = Instant::now();
//!
//!     metrics!(timing, "process.query_time", start, end);
//!     metrics!(counter, "process.query_row_count", row_count);
//!
//!     row_count
//! }
//! # fn main() {}
//! ```

use std::time::{Duration, Instant};

pub use metrics::{self as internal, SetRecorderError};

/// A simple timer which is used to time how much time elapsed.
pub struct Timer(Instant);

impl Timer {
    /// Starts a new timer.
    pub fn start() -> Self {
        Self(Instant::now())
    }

    /// Stops the timer and return how much time elapsed.
    pub fn stop(self) -> Duration {
        Instant::now() - self.0
    }
}

/// Reexports the macros from the crate `metrics`.
///
/// See the list of available [metrics types](https://docs.rs/metrics/*/metrics/index.html#macros).
#[macro_export(local_inner_macros)]
macro_rules! metrics {
    ($type:ident, $( $args:tt )*) => {
        $crate::internal::$type!($( $args )*);
    }
}
