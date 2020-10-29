//! TODO(doc): @yangby-cryptape
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/* Examples:
 * ```toml
 * [metrics]
 * threads = 3
 * histogram_window = 60
 * histogram_granularity = 1
 * upkeep_interval = 500
 * [metrics.exporter.prometheus]
 * target = { type = "http", listen_address = "127.0.0.1:8100" }
 * format = { type = "prometheus" }
 * [metrics.exporter.log_yaml]
 * target = { type = "log", level = "warn", interval = 600 }
 * format = { type = "yaml" }
 * [metrics.exporter.log_json]
 * target = { type = "log", level = "error", interval = 900 }
 * format = { type = "json" }
 * ```
 */
/// TODO(doc): @yangby-cryptape
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// TODO(doc): @yangby-cryptape
    #[serde(default)]
    pub threads: usize,

    /// TODO(doc): @yangby-cryptape
    #[serde(default)]
    pub histogram_window: u64, // seconds
    /// TODO(doc): @yangby-cryptape
    #[serde(default)]
    pub histogram_granularity: u64, // seconds
    /// TODO(doc): @yangby-cryptape
    #[serde(default)]
    pub upkeep_interval: u64, // milliseconds

    /// TODO(doc): @yangby-cryptape
    #[serde(default)]
    pub exporter: HashMap<String, Exporter>,
}

/// TODO(doc): @yangby-cryptape
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Exporter {
    /// TODO(doc): @yangby-cryptape
    pub target: Target,
    /// TODO(doc): @yangby-cryptape
    pub format: Format,
}

/// TODO(doc): @yangby-cryptape
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum Target {
    /// TODO(doc): @yangby-cryptape
    Log {
        /// TODO(doc): @yangby-cryptape
        level: LogLevel,
        /// TODO(doc): @yangby-cryptape
        interval: u64, // seconds
    },
    /// TODO(doc): @yangby-cryptape
    Http {
        /// TODO(doc): @yangby-cryptape
        listen_address: String,
    },
}

/// TODO(doc): @yangby-cryptape
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum Format {
    /// TODO(doc): @yangby-cryptape
    Json {
        /// TODO(doc): @yangby-cryptape
        #[serde(default)]
        pretty: bool,
    },
    /// TODO(doc): @yangby-cryptape
    Yaml,
    /// TODO(doc): @yangby-cryptape
    Prometheus,
}

/// TODO(doc): @yangby-cryptape
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// TODO(doc): @yangby-cryptape
    Error,
    /// TODO(doc): @yangby-cryptape
    Warn,
    /// TODO(doc): @yangby-cryptape
    Info,
    /// TODO(doc): @yangby-cryptape
    Debug,
    /// TODO(doc): @yangby-cryptape
    Trace,
}

impl From<LogLevel> for log::Level {
    fn from(lv: LogLevel) -> Self {
        match lv {
            LogLevel::Error => Self::Error,
            LogLevel::Warn => Self::Warn,
            LogLevel::Info => Self::Info,
            LogLevel::Debug => Self::Debug,
            LogLevel::Trace => Self::Trace,
        }
    }
}
