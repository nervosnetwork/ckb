use serde::{Deserialize, Serialize};
/// TODO(doc): @doitian
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Config {
    /// TODO(doc): @doitian
    pub new_block_notify_script: Option<String>,
    /// TODO(doc): @doitian
    pub network_alert_notify_script: Option<String>,
}
