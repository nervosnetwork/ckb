pub use log::{self as internal, Level, SetLoggerError};

#[doc(hidden)]
#[macro_export]
macro_rules! env {
    ($($inner:tt)*) => {
        env!($($inner)*)
    }
}

#[macro_export(local_inner_macros)]
macro_rules! trace {
    ($( $args:tt )*) => {
        $crate::internal::trace!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! debug {
    ($( $args:tt )*) => {
        $crate::internal::debug!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! info {
    ($( $args:tt )*) => {
        $crate::internal::info!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! warn {
    ($( $args:tt )*) => {
        $crate::internal::warn!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! error {
    ($( $args:tt )*) => {
        $crate::internal::error!($( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! log_enabled {
    ($level:expr) => {
        $crate::internal::log_enabled!($level);
    };
}

#[macro_export(local_inner_macros)]
macro_rules! trace_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::trace!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! debug_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::debug!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! info_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::info!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! warn_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::warn!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! error_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::error!(target: $target, $( $args )*);
    }
}

#[macro_export(local_inner_macros)]
macro_rules! log_enabled_target {
    ($target:expr, $level:expr) => {
        $crate::internal::log_enabled!(target: $target, $level);
    };
}
