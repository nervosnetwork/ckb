use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub struct ExtraLoggerConfig {
    pub filter: String,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub struct MainLoggerConfig {
    pub filter: Option<String>,
    pub to_stdout: Option<bool>,
    pub to_file: Option<bool>,
    pub color: Option<bool>,
}
