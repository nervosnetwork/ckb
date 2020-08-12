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
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub threads: usize,

    #[serde(default)]
    pub histogram_window: u64, // seconds
    #[serde(default)]
    pub histogram_granularity: u64, // seconds
    #[serde(default)]
    pub upkeep_interval: u64, // milliseconds

    #[serde(default)]
    pub exporter: HashMap<String, Exporter>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Exporter {
    pub target: Target,
    pub format: Format,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum Target {
    Log {
        level: LogLevel,
        interval: u64, // seconds
    },
    Http {
        listen_address: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum Format {
    Json {
        #[serde(default)]
        pretty: bool,
    },
    Yaml,
    Prometheus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
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
