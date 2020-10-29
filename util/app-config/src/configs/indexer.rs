use crate::DBConfig;
use serde::{Deserialize, Serialize};

/// Indexer configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// The minimum time (in milliseconds) between indexing execution, default is 500
    pub batch_interval: u64,
    /// The maximum number of blocks in a single indexing execution batch, default is 200
    pub batch_size: usize,
    /// TODO(doc): @doitian
    pub db: DBConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            batch_interval: 500,
            batch_size: 200,
            db: Default::default(),
        }
    }
}
