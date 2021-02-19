use serde::{Deserialize, Serialize};
/// Notify config options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Config {
    /// An executable script to be called whenever there's a new block in the canonical chain.
    ///
    /// The script is called with the block hash as the argument.
    pub new_block_notify_script: Option<String>,
    /// An executable script to be called whenever there's a new network alert received.
    ///
    /// The script is called with the alert message as the argument.
    pub network_alert_notify_script: Option<String>,
}
