use crate::error::MyError;
use crate::types::TextInfo;
use crate::yaml_processor::load_yaml;

use std::fs::read_dir;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;

pub fn backfill(input_dir: &PathBuf) {
    if let Ok(text_info_lists) = load_all_text_info_files(input_dir) {
        backfill_edited(text_info_lists)
    } else {
        log::error!("Backfill failed to start, please fix text info file first.");
    }
}

fn load_all_text_info_files(input_dir: &PathBuf) -> Result<Vec<(String, Vec<TextInfo>)>, MyError> {
    let mut text_info_lists: Vec<(String, Vec<TextInfo>)> = vec![];
    let entries = read_dir(input_dir).expect("Read text info file failed.");
    for entry in entries.flatten() {
        if let Some(file_name) = entry.file_name().to_str() {
            if file_name.ends_with(".yml") {
                let entry_path = entry.path();
                let list = load_yaml(&entry_path).expect("load yaml");
                text_info_lists.push((file_name.to_owned(), list))
            }
        }
    }
    Ok(text_info_lists)
}

fn backfill_edited(text_info_lists: Vec<(String, Vec<TextInfo>)>) {
    for list in text_info_lists {
        log::info!("Parse text info file: {:?}", list.0);
        for text_info in list.1 {
            let mut source_code = String::new();
            let mut file = File::open(text_info.metadata().file()).expect("Failed to open file");
            file.read_to_string(&mut source_code)
                .expect("Failed to read file");

            // Replace the match origianl with the edited
            let new_source_code = source_code.replace(text_info.original(), text_info.editable());

            // Reopen the file in write mode and write the new source code
            let mut new_file =
                File::create(text_info.metadata().file()).expect("Failed to create file");
            new_file
                .write_all(new_source_code.as_bytes())
                .expect("Failed to write file");
        }
    }
    log::info!("The backfill is completed, please review the modifications in the source code.");
}
