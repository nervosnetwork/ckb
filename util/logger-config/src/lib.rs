//! CKB logger configurations.
//!
//! This crate is used to configure the [CKB logger and logging service].
//!
//! [CKB logger and logging service]: ../ckb_logger_service/index.html

use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

/// The whole CKB logger configuration.
///
/// This struct is used to build [`Logger`].
///
/// Include configurations of the main logger and any number of extra loggers.
///
/// [`Logger`]: ../ckb_logger_service/struct.Logger.html
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// An optional string which is used to build [env_logger::Filter] for the main logger.
    ///
    /// If the value is `None`, no [env_logger::Filter] will be used.
    ///
    /// [env_logger::Filter]: https://docs.rs/env_logger/*/env_logger/filter/struct.Filter.html
    pub filter: Option<String>,
    /// Colorize the output which was written into the stdout.
    #[serde(default = "default_values::color")]
    pub color: bool,
    /// The log file of the main loggger.
    #[serde(skip)]
    pub file: PathBuf,
    /// The directory where to store all log files.
    #[serde(skip)]
    pub log_dir: PathBuf,
    /// Output the log records of the main logger into a file or not.
    #[serde(default = "default_values::log_to_file")]
    pub log_to_file: bool,
    /// Output the log records of the main logger into the stdout or not.
    #[serde(default = "default_values::log_to_stdout")]
    pub log_to_stdout: bool,
    /// An optional bool to control whether or not emit [Sentry Breadcrumbs].
    ///
    /// if the value is `None`, not emit [Sentry Breadcrumbs].
    ///
    /// [Sentry Breadcrumbs]: https://sentry.io/features/breadcrumbs/
    pub emit_sentry_breadcrumbs: Option<bool>,
    /// Add extra loggers.
    #[serde(default)]
    pub extra: HashMap<String, ExtraLoggerConfig>,
}

/// The configuration of an extra CKB logger.
///
/// This struct is used to build [`ExtraLogger`].
///
/// [`ExtraLogger`]: ../ckb_logger_service/struct.ExtraLogger.html
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtraLoggerConfig {
    /// A string which is used to build [env_logger::Filter] for the extra logger.
    ///
    /// [env_logger::Filter]: https://docs.rs/env_logger/*/env_logger/filter/struct.Filter.html
    pub filter: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            filter: None,
            color: default_values::color(),
            file: Default::default(),
            log_dir: Default::default(),
            log_to_file: default_values::log_to_file(),
            log_to_stdout: default_values::log_to_stdout(),
            emit_sentry_breadcrumbs: None,
            extra: Default::default(),
        }
    }
}

pub(crate) mod default_values {
    pub(crate) const fn color() -> bool {
        !cfg!(windows)
    }

    pub(crate) const fn log_to_file() -> bool {
        false
    }

    pub(crate) const fn log_to_stdout() -> bool {
        true
    }
}
