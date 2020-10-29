use serde::{Deserialize, Serialize};

/// TODO(doc): @doitian
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// TODO(doc): @doitian
    pub interval: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self { interval: 0 }
    }
}
