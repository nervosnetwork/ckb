use ckb_jsonrpc_types::JsonBytes;
use serde::{Deserialize, Serialize};

/// TODO(doc): @doitian
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// TODO(doc): @doitian
    pub signatures_threshold: usize,
    /// TODO(doc): @doitian
    pub public_keys: Vec<JsonBytes>,
}

impl Default for Config {
    fn default() -> Self {
        let alert_config = include_bytes!("./alert_signature.toml");
        toml::from_slice(&alert_config[..]).expect("alert system config")
    }
}
