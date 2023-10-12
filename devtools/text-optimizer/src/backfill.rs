use crate::yaml_processor::load_yaml;

use std::fs::read_dir;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;

pub fn backfill(input_dir: &PathBuf) {
    if let Ok(entries) = read_dir(input_dir) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if let Some(file_name) = entry.file_name().to_str() {
                if file_name.ends_with(".yml") {
                    backfill_by_text_info(&entry_path);
                    log::trace!("{:#?}", entry_path);
                }
            }
        }
    }
}

fn backfill_by_text_info(file_path: &PathBuf) {
    let log_text_list = load_yaml(&file_path).expect("load yaml");
    for text_info in log_text_list {
        let mut source_code = String::new();
        let mut file = File::open(text_info.metadata().file()).expect("Failed to open file");
        file.read_to_string(&mut source_code)
            .expect("Failed to read file");

        // Replace the match with the new string
        let new_source_code = source_code.replace(text_info.original(), text_info.editable());

        // Reopen the file in write mode and write the new source code
        let mut new_file =
            File::create(&text_info.metadata().file()).expect("Failed to create file");
        new_file
            .write_all(new_source_code.as_bytes())
            .expect("Failed to write file");
    }
}
