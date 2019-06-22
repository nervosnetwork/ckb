use ckb_jsonrpc_types::JsonBytes;
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub signatures_threshold: usize,
    pub public_keys: Vec<JsonBytes>,
}

impl Default for Config {
    fn default() -> Self {
        let alert_config = include_bytes!("./alert.toml");
        toml::from_slice(&alert_config[..]).expect("alert system config")
    }
}
