use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub filter: Option<String>,
    pub color: bool,
    #[serde(skip)]
    pub file: PathBuf,
    #[serde(skip)]
    pub log_dir: PathBuf,
    pub log_to_file: bool,
    pub log_to_stdout: bool,
    pub emit_sentry_breadcrumbs: Option<bool>,
    #[serde(default)]
    pub extra: HashMap<String, ExtraLoggerConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtraLoggerConfig {
    pub filter: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            filter: None,
            color: !cfg!(windows),
            file: Default::default(),
            log_dir: Default::default(),
            log_to_file: false,
            log_to_stdout: true,
            emit_sentry_breadcrumbs: None,
            extra: Default::default(),
        }
    }
}
