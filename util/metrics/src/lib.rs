use std::time::{Duration, Instant};

pub use metrics::{self as internal, SetRecorderError};

pub struct Timer(Instant);

impl Timer {
    pub fn start() -> Self {
        Self(Instant::now())
    }

    pub fn stop(self) -> Duration {
        Instant::now() - self.0
    }
}

// Ref: https://docs.rs/metrics/*/metrics/index.html#macros
#[macro_export(local_inner_macros)]
macro_rules! metrics {
    ($type:ident, $( $args:tt )*) => {
        $crate::internal::$type!($( $args )*);
    }
}
