use serde::{Deserialize, Serialize};

/// Memory tracker config options.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// Tracking interval in seconds.
    pub interval: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self { interval: 0 }
    }
}
