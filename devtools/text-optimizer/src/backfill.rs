use crate::yaml_processor::load_yaml;

use std::fs::read_dir;
use std::path::PathBuf;

pub fn backfill(input_dir: &PathBuf) {
    if let Ok(entries) = read_dir(input_dir) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if let Some(file_name) = entry.file_name().to_str() {
                if file_name.ends_with(".yml") {
                    log::trace!("{:#?}", entry_path);
                    let _log_text_list = load_yaml(&entry_path).expect("load yaml");
                }
            }
        }
    }
}
