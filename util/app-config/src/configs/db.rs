use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub path: PathBuf,
    #[serde(default)]
    pub options: HashMap<String, String>,
}

impl Config {
    pub fn set_default_for_empty_options(&mut self) {
        self.set_option_if_empty("total_threads", num_cpus::get().to_string());
        self.set_option_if_empty("write_buffer_size", format!("{}", 16 << 20));
    }

    fn set_option_if_empty(&mut self, key: &str, value: String) {
        self.options.entry(key.to_owned()).or_insert(value);
    }
}
