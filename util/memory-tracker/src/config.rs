use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub interval: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self { interval: 0 }
    }
}
