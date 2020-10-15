//! CKB metrics configurations.
//!
//! This crate is used to configure the [CKB metrics service].
//!
//! [CKB metrics service]: ../ckb_metrics_service/index.html

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub use log::Level as LogLevel;

/// The whole CKB metrics configuration.
///
/// This struct is used to configure [CKB metrics service]:
/// builds one [`metrics_runtime::Receiver`] and any number of [exporters]
///
/// # An example which is used in `ckb.toml`:
/// ```toml
/// [metrics]
/// threads = 3
/// histogram_window = 60
/// histogram_granularity = 1
/// upkeep_interval = 500
/// [metrics.exporter.prometheus]
/// target = { type = "http", listen_address = "127.0.0.1:8100" }
/// format = { type = "prometheus" }
/// [metrics.exporter.log_yaml]
/// target = { type = "log", level = "warn", interval = 600 }
/// format = { type = "yaml" }
/// [metrics.exporter.log_json]
/// target = { type = "log", level = "error", interval = 900 }
/// format = { type = "json" }
/// ```
///
/// [CKB metrics service]: ../ckb_metrics_service/index.html
/// [`metrics_runtime::Receiver`]: https://docs.rs/metrics-runtime/0.13.1/metrics_runtime/struct.Receiver.html
/// [exporters]: https://docs.rs/metrics-runtime/0.13.1/metrics_runtime/exporters/index.html
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// How many threads are required for metrics service.
    #[serde(default)]
    pub threads: usize,

    /// Sets the [histogram] window configuration in seconds.
    ///
    /// [histogram]: https://docs.rs/metrics-runtime/0.13.1/metrics_runtime/struct.Builder.html#method.histogram
    #[serde(default)]
    pub histogram_window: u64,
    /// Sets the [histogram] granularity configuration in seconds.
    ///
    /// [histogram]: https://docs.rs/metrics-runtime/0.13.1/metrics_runtime/struct.Builder.html#method.histogram
    #[serde(default)]
    pub histogram_granularity: u64,
    /// Sets the [upkeep interval] configuration in milliseconds.
    ///
    /// [upkeep interval]: https://docs.rs/metrics-runtime/0.13.1/metrics_runtime/struct.Builder.html#method.upkeep_interval
    #[serde(default)]
    pub upkeep_interval: u64,

    /// Stores all exporters configurations.
    #[serde(default)]
    pub exporter: HashMap<String, Exporter>,
}

/// The configuration of an [exporter].
///
/// [exporter]: https://docs.rs/metrics-runtime/0.13.1/metrics_runtime/exporters/index.html
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Exporter {
    /// How to output the metrics data.
    pub target: Target,
    /// The metrics output data in which format.
    pub format: Format,
}

/// The target to output the metrics data.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum Target {
    /// Outputs the metrics data into logs.
    Log {
        /// The log records will be output at which level.
        level: LogLevel,
        /// Outputs each log record after how many seconds.
        interval: u64,
    },
    /// Outputs the metrics data through HTTP Protocol.
    Http {
        /// The HTTP listen address.
        listen_address: String,
    },
}

/// Records the metrics data in which format.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum Format {
    /// JSON format.
    Json {
        /// Sets whether or not to render the JSON as "pretty."
        #[serde(default)]
        pretty: bool,
    },
    /// YAML format.
    Yaml,
    /// Prometheus exposition format.
    Prometheus,
}
