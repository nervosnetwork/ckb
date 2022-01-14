//! A lightweight metrics facade used in CKB.
//!
//! The `ckb-metrics` crate is a set of tools for metrics.
//! The crate [`ckb-metrics-service`] is the runtime which handles the metrics data in CKB.
//!
//! [`ckb-metrics-service`]: ../ckb_metrics_service/index.html
//!
//! ## Use
//!
//! The basic use of the facade crate is through the metrics macro: [`metrics!`].
//!
//! ### Examples
//!
//! ```rust
//! use ckb_metrics::{metrics, Timer};
//!
//! fn run_query(_: &str) -> u64 { 42 }
//!
//! pub fn process(query: &str) -> u64 {
//!     let timer = Timer::start();
//!     let row_count = run_query(query);
//!     metrics!(timing, "process.query_time", timer.stop());
//!     metrics!(counter, "process.query_row_count", row_count);
//!     row_count
//! }
//! ```

use opentelemetry::metrics::Meter;
use std::time::{Duration, Instant};

#[doc(hidden)]
pub use opentelemetry as internal;

/// Returns a global meter.
pub fn global_meter() -> Meter {
    opentelemetry::global::meter("ckb-metrics")
}

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

/// Out-of-the-box macros for metrics.
///
/// - A counter is a cumulative metric that represents a monotonically increasing value which
///   can only be increased or be reset to zero on restart.
/// - A gauge is a metric that can go up and down, arbitrarily, over time.
/// - A timing is a metric of time consumed.
// Since the APIs of opentelemetry<=0.15.0 is not stable, so just let them be compatible with metrics=0.12.1.
#[macro_export(local_inner_macros)]
macro_rules! metrics {
    (counter, $label:literal, $value:expr $(, $span_name:expr => $span_value:expr )* $(,)?) => {
        $crate::global_meter()
            .u64_counter($label)
            .init()
            .add($value, &[$( $crate::internal::KeyValue::new($span_name, $span_value), )*]);
    };
    (gauge, $label:literal, $value:expr $(, $span_name:expr => $span_value:expr )* $(,)?) => {
        $crate::global_meter()
            .i64_up_down_counter($label)
            .init()
            .add($value, &[$( $crate::internal::KeyValue::new($span_name, $span_value), )*]);
    };
    (timing, $label:literal, $duration:expr $(, $span_name:expr => $span_value:expr )* $(,)?) => {
        $crate::global_meter()
            .f64_value_recorder($label)
            .init()
            .record($duration.as_secs_f64(), &[$( $crate::internal::KeyValue::new($span_name, $span_value), )*]);
    };
}
