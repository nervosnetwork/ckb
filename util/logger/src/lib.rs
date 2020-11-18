//! CKB logging facade.
//!
//! This crate is a wrapper of the crate [`log`].
//!
//! [`log`]: https://docs.rs/log/*/log/index.html
//!
//! The major issue of the crate `log` is that the macro like
//! `trace!(target: "global", "message")` is unfriendly to `cargo fmt`. So this
//! crate disallow using `target: ` in the basic logging macros and add another
//! group of macros to support both target and message, for example,
//! `trace_target!("global", "message")`.
pub use log::{self as internal, Level, SetLoggerError};

#[doc(hidden)]
#[macro_export]
macro_rules! env {
    ($($inner:tt)*) => {
        env!($($inner)*)
    }
}

/// Logs a message at the trace level using the default target.
///
/// This macro logs the message using the default target, the module path of
/// the location of the log request. See [`trace_target!`] which can override the
/// target.
///
/// [`trace_target!`]: macro.trace_target.html
///
/// # Examples
///
/// ```
/// use ckb_logger::trace;
///
/// # struct Position { x: f32, y: f32 }
/// let pos = Position { x: 3.234, y: -1.223 };
///
/// trace!("Position is: x: {}, y: {}", pos.x, pos.y);
/// ```
#[macro_export(local_inner_macros)]
macro_rules! trace {
    ($( $args:tt )*) => {
        $crate::internal::trace!($( $args )*);
    }
}

/// Logs a message at the debug level using the default target.
///
/// This macro logs the message using the default target, the module path of
/// the location of the log request. See [`debug_target!`] which can override the
/// target.
///
/// [`debug_target!`]: macro.debug_target.html
///
/// # Examples
///
/// ```
/// use ckb_logger::debug;
///
/// # struct Position { x: f32, y: f32 }
/// let pos = Position { x: 3.234, y: -1.223 };
///
/// debug!("Position is: x: {}, y: {}", pos.x, pos.y);
/// ```
#[macro_export(local_inner_macros)]
macro_rules! debug {
    ($( $args:tt )*) => {
        $crate::internal::debug!($( $args )*);
    }
}

/// Logs a message at the info level using the default target.
///
/// This macro logs the message using the default target, the module path of
/// the location of the log request. See [`info_target!`] which can override the
/// target.
///
/// [`info_target!`]: macro.info_target.html
///
/// # Examples
///
/// ```
/// use ckb_logger::info;
///
/// # struct Connection { port: u32, speed: f32 }
/// let conn_info = Connection { port: 40, speed: 3.20 };
///
/// info!("Connected to port {} at {} Mb/s", conn_info.port, conn_info.speed);
/// info!(target: "connection_events", "Successfull connection, port: {}, speed: {}",
///       conn_info.port, conn_info.speed);
/// ```
#[macro_export(local_inner_macros)]
macro_rules! info {
    ($( $args:tt )*) => {
        $crate::internal::info!($( $args )*);
    }
}

/// Logs a message at the warn level using the default target.
///
/// This macro logs the message using the default target, the module path of
/// the location of the log request. See [`warn_target!`] which can override the
/// target.
///
/// [`warn_target!`]: macro.warn_target.html
///
/// # Examples
///
/// ```
/// use ckb_logger::warn;
///
/// let warn_description = "Invalid Input";
///
/// warn!("Warning! {}!", warn_description);
/// ```
#[macro_export(local_inner_macros)]
macro_rules! warn {
    ($( $args:tt )*) => {
        $crate::internal::warn!($( $args )*);
    }
}

/// Logs a message at the error level using the default target.
///
/// This macro logs the message using the default target, the module path of
/// the location of the log request. See [`error_target!`] which can override the
/// target.
///
/// [`error_target!`]: macro.error_target.html
///
/// # Examples
///
/// ```
/// use ckb_logger::error;
///
/// let (err_info, port) = ("No connection", 22);
///
/// error!("Error: {} on port {}", err_info, port);
/// ```
#[macro_export(local_inner_macros)]
macro_rules! error {
    ($( $args:tt )*) => {
        $crate::internal::error!($( $args )*);
    }
}

/// Determines if a message logged at the specified level and with the default target will be logged.
///
/// The default target is the module path of the location of the log request.
/// See also [`log_enabled_target!`] the version that supports checking arbitrary
/// target.
///
/// [`log_enabled_target!`]: macro.log_enabled_target.html
///
/// This can be used to avoid expensive computation of log message arguments if the message would be ignored anyway.
///
/// ## Examples
///
/// ```
/// use ckb_logger::Level::Debug;
/// use ckb_logger::{debug, log_enabled};
///
/// # struct Data { x: u32, y: u32 }
/// # fn expensive_call() -> Data { Data { x: 0, y: 0 } }
/// if log_enabled!(Debug) {
///     let data = expensive_call();
///     debug!("expensive debug data: {} {}", data.x, data.y);
/// }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! log_enabled {
    ($level:expr) => {
        $crate::internal::log_enabled!($level);
    };
}

/// Logs a message at the trace level using the specified target.
///
/// This macro logs the message using the specified target. In the most
/// scenarios, the log message should just use the default target, which is the
/// module path of the location of the log request. See [`trace!`] which just logs
/// using the default target.
///
/// [`trace!`]: macro.trace.html
///
/// # Examples
///
/// ```
/// use ckb_logger::trace_target;
///
/// # struct Position { x: f32, y: f32 }
/// let pos = Position { x: 3.234, y: -1.223 };
///
/// trace_target!("app_events", "Position is: x: {}, y: {}", pos.x, pos.y);
/// ```
#[macro_export(local_inner_macros)]
macro_rules! trace_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::trace!(target: $target, $( $args )*);
    }
}

/// Logs a message at the debug level using the specified target.
///
/// This macro logs the message using the specified target. In the most
/// scenarios, the log message should just use the default target, which is the
/// module path of the location of the log request. See [`debug!`] which just logs
/// using the default target.
///
/// [`debug!`]: macro.debug.html
///
/// # Examples
///
/// ```
/// use ckb_logger::debug_target;
///
/// # struct Position { x: f32, y: f32 }
/// let pos = Position { x: 3.234, y: -1.223 };
///
/// debug_target!("app_events", "Position is: x: {}, y: {}", pos.x, pos.y);
/// ```
#[macro_export(local_inner_macros)]
macro_rules! debug_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::debug!(target: $target, $( $args )*);
    }
}

/// Logs a message at the info level using the specified target.
///
/// This macro logs the message using the specified target. In the most
/// scenarios, the log message should just use the default target, which is the
/// module path of the location of the log request. See [`info!`] which just logs
/// using the default target.
///
/// [`info!`]: macro.info.html
///
/// # Examples
///
/// ```
/// use ckb_logger::info_target;
///
/// # struct Connection { port: u32, speed: f32 }
/// let conn_info = Connection { port: 40, speed: 3.20 };
///
/// info_target!("connection_events", "Successfull connection, port: {}, speed: {}",
///       conn_info.port, conn_info.speed);
/// ```
#[macro_export(local_inner_macros)]
macro_rules! info_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::info!(target: $target, $( $args )*);
    }
}

/// Logs a message at the warn level using the specified target.
///
/// This macro logs the message using the specified target. In the most
/// scenarios, the log message should just use the default target, which is the
/// module path of the location of the log request. See [`warn!`] which just logs
/// using the default target.
///
/// [`warn!`]: macro.warn.html
///
/// # Examples
///
/// ```
/// use ckb_logger::warn_target;
///
/// let warn_description = "Invalid Input";
///
/// warn_target!("input_events", "App received warning: {}", warn_description);
/// ```
#[macro_export(local_inner_macros)]
macro_rules! warn_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::warn!(target: $target, $( $args )*);
    }
}

/// Logs a message at the error level using the specified target.
///
/// This macro logs the message using the specified target. In the most
/// scenarios, the log message should just use the default target, which is the
/// module path of the location of the log request. See [`error!`] which just logs
/// using the default target.
///
/// [`error!`]: macro.error.html
///
/// # Examples
///
/// ```
/// use ckb_logger::error_target;
///
/// let (err_info, port) = ("No connection", 22);
///
/// error_target!("app_events", "App Error: {}, Port: {}", err_info, port);
/// ```
#[macro_export(local_inner_macros)]
macro_rules! error_target {
    ($target:expr, $( $args:tt )*) => {
        $crate::internal::error!(target: $target, $( $args )*);
    }
}

/// Determines if a message logged at the specified level and with the specified target will be logged.
///
/// This can be used to avoid expensive computation of log message arguments if the message would be ignored anyway.
///
/// See also [`log_enabled!`] the version that checks with the default target.
///
/// [`log_enabled!`]: macro.log_enabled.html
///
/// ## Examples
///
/// ```
/// use ckb_logger::Level::Debug;
/// use ckb_logger::{debug_target, log_enabled_target};
///
/// # struct Data { x: u32, y: u32 }
/// # fn expensive_call() -> Data { Data { x: 0, y: 0 } }
/// if log_enabled_target!("Global", Debug) {
///     let data = expensive_call();
///     debug_target!("Global", "expensive debug data: {} {}", data.x, data.y);
/// }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! log_enabled_target {
    ($target:expr, $level:expr) => {
        $crate::internal::log_enabled!(target: $target, $level);
    };
}
