use ckb_jsonrpc_types::JsonBytes;
use serde::{Deserialize, Serialize};

/// Network alert config options.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// The minimual number of required signatures to send a network alert.
    pub signatures_threshold: usize,
    /// The public keys of all the network alert signers.
    pub public_keys: Vec<JsonBytes>,
}

impl Default for Config {
    fn default() -> Self {
        let alert_config = include_bytes!("./alert_signature.toml");
        toml::from_slice(&alert_config[..]).expect("alert system config")
    }
}
