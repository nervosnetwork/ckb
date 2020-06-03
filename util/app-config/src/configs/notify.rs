use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Config {
    pub new_block_notify_script: Option<String>,
    pub network_alert_notify_script: Option<String>,
}
