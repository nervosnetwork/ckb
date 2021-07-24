//! CKB metrics configurations.
//!
//! This crate is used to configure the [CKB metrics service].
//!
//! [CKB metrics service]: ../ckb_metrics_service/index.html

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The whole CKB metrics configuration.
///
/// This struct is used to configure [CKB metrics service].
///
/// # An example which is used in `ckb.toml`:
/// ```toml
/// [metrics.exporter.prometheus]
/// target = { type = "prometheus", listen_address = "127.0.0.1:8100" }
/// ```
///
/// [CKB metrics service]: ../ckb_metrics_service/index.html
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Stores all exporters configurations.
    #[serde(default)]
    pub exporter: HashMap<String, Exporter>,
}

/// The configuration of an exporter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Exporter {
    /// How to output the metrics data.
    pub target: Target,
}

/// The target to output the metrics data.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum Target {
    /// Outputs the metrics data through Prometheus.
    Prometheus {
        /// The HTTP listen address.
        listen_address: String,
    },
}
