//! TODO(doc): @yangby-cryptape
use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
/// TODO(doc): @yangby-cryptape
pub struct Config {
    /// TODO(doc): @yangby-cryptape
    pub filter: Option<String>,
    /// TODO(doc): @yangby-cryptape
    pub color: bool,
    /// TODO(doc): @yangby-cryptape
    #[serde(skip)]
    pub file: PathBuf,
    /// TODO(doc): @yangby-cryptape
    #[serde(skip)]
    pub log_dir: PathBuf,
    /// TODO(doc): @yangby-cryptape
    pub log_to_file: bool,
    /// TODO(doc): @yangby-cryptape
    pub log_to_stdout: bool,
    /// TODO(doc): @yangby-cryptape
    pub emit_sentry_breadcrumbs: Option<bool>,
    /// TODO(doc): @yangby-cryptape
    #[serde(default)]
    pub extra: HashMap<String, ExtraLoggerConfig>,
}

/// TODO(doc): @yangby-cryptape
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtraLoggerConfig {
    /// TODO(doc): @yangby-cryptape
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
