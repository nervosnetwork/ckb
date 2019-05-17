use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub orphan_block_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            orphan_block_limit: 1024,
        }
    }
}
