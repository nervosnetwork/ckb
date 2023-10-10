use crate::yaml_processor::load_yaml;
use crate::LOG_TEXT_FILE;

use std::path::PathBuf;
use std::{
    fs::File,
    io::{BufRead, BufReader},
};

pub fn backfill(input_dir: &PathBuf) {
    let log_text_file = input_dir.to_owned().join(LOG_TEXT_FILE);
    let log_text_list = load_yaml(&log_text_file).expect("load yaml");
    println!("{:#?}", log_text_list);

    for log_text in log_text_list {
        let file_path = log_text.metadata().file();
        let code_line = log_text.metadata().start_line();

        let file = File::open(file_path).expect("open file");
        let reader = BufReader::new(file);
        let lines = reader
            .lines()
            .map(|line| line.unwrap())
            .collect::<Vec<String>>();

        let _start_line = code_line.saturating_sub(3); // Ensure start_line is at least 0
        let _end_line = (code_line + 3).min(lines.len());

        // for i in start_line..end_line {
        //     if lines[i].contains(&log_text.original()) {
        //         // Replace the line containing the original text
        //         lines[i] = lines[i].replace(&log_info.original, &log_info.editable);
        //     }
        // }

        // let updated_content = lines.join("\n");

        // // Write the updated content back to the file
        // let mut file = File::create(file_path)?;
        // file.write_all(updated_content.as_bytes())?;
    }
}
