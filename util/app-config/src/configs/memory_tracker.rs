use serde::{Deserialize, Serialize};

/// Memory tracker config options.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Tracking interval in seconds.
    pub interval: u64,
}
