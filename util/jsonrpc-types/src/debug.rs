use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub struct ExtraLoggerConfig {
    pub filter: String,
}
