use ckb_jsonrpc_types::JsonBytes;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignatureConfig {
    pub signatures_threshold: usize,
    pub public_keys: Vec<JsonBytes>,
}

impl Default for SignatureConfig {
    fn default() -> Self {
        let alert_config = include_bytes!("./alert_signature.toml");
        toml::from_slice(&alert_config[..]).expect("alert system config")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct NotifierConfig {
    pub notify_script: Option<String>,
}
