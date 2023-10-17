pub mod clap_extractor;
pub mod log_extractor;
pub mod std_output_extractor;
pub mod thiserror_extractor;

use crate::{
    error::MyError, types::TextInfo, yaml_processor::save_yaml, CLAP_TEXT_FILE, LOG_TEXT_FILE,
    STD_OUTPUT_TEXT_FILE, THISERROR_TEXT_FILE,
};

use clap_extractor::ClapExtractor;
use log_extractor::LogExtractor;
use std_output_extractor::StdOutputExtractor;
use thiserror_extractor::ThiserrorExtractor;

use cargo_metadata::MetadataCommand;
use syn::File;

use std::{
    fs,
    path::{Path, PathBuf},
};

pub trait Extractor {
    fn reset_scanning_path(&mut self, file_path: &Path);
    fn add_text_info(&mut self, text_info: TextInfo);
    fn text_list(&self) -> Vec<TextInfo>;
    fn scanning_file_path(&self) -> &PathBuf;
    fn save_file_path(&self) -> &PathBuf;
    fn visit_file(&mut self, node: &File);
}

pub fn extract(root_cargo_config_path: PathBuf, commit_id: &str, output_dir: &PathBuf) {
    // init all extractors
    let mut clap_extractor = ClapExtractor::new(output_dir.join(CLAP_TEXT_FILE));
    let mut log_extractor = LogExtractor::new(output_dir.join(LOG_TEXT_FILE));
    let mut std_output_extractor = StdOutputExtractor::new(output_dir.join(STD_OUTPUT_TEXT_FILE));
    let mut thiserror_extractor = ThiserrorExtractor::new(output_dir.join(THISERROR_TEXT_FILE));

    let mut extractors: Vec<&mut dyn Extractor> = vec![
        &mut clap_extractor,
        &mut log_extractor,
        &mut std_output_extractor,
        &mut thiserror_extractor,
    ];

    let project_metadata = MetadataCommand::new()
        .manifest_path(&root_cargo_config_path)
        .exec()
        .expect("Failed to get current directory");

    for package in project_metadata.workspace_packages() {
        log::info!("Scanning Crate: {}", package.name);

        let crate_src_path = PathBuf::from(&package.manifest_path)
            .parent()
            .expect("workspace member crate path")
            .join("src");
        let crate_src_path =
            to_relative_path(&crate_src_path, &root_cargo_config_path).expect("to_relative_path");

        process_rs_files_in_src(&crate_src_path, &mut extractors);
    }

    save_text_info_files(output_dir, &extractors, commit_id);
}

pub fn process_rs_files_in_src(dir_path: &Path, extractors: &mut [&mut dyn Extractor]) {
    if let Ok(entries) = fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                process_rs_files_in_src(&entry_path, extractors);
            } else if let Some(file_name) = entry.file_name().to_str() {
                if file_name.ends_with(".rs") {
                    log::trace!("Found .rs file: {:?}", entry_path);

                    let file_content =
                        fs::read_to_string(&entry_path).expect("Failed to read file");

                    for extractor in extractors.iter_mut() {
                        extractor.reset_scanning_path(&entry_path);

                        if let Ok(syntax_tree) = syn::parse_file(&file_content) {
                            extractor.visit_file(&syntax_tree)
                        } else {
                            log::error!("Failed to parse .rs file: {:?}", entry_path);
                        }
                    }
                }
            }
        }
    }
}

pub fn extract_contents_in_brackets(lit: String) -> Option<String> {
    if let Some(start) = lit.find('"') {
        if let Some(end) = lit.rfind('"') {
            let format_string = &lit[start + 1..end];
            return Some(format_string.to_string());
        }
    }
    None
}

fn save_text_info_files(output_dir: &PathBuf, extractors: &[&mut dyn Extractor], commit_id: &str) {
    fs::create_dir_all(output_dir).expect("create dir all");
    println!();

    for extractor in extractors {
        save_yaml(
            extractor.save_file_path(),
            &extractor.text_list(),
            commit_id,
        )
        .expect("save yaml");

        let file_name = extractor.save_file_path().file_name().unwrap();
        let text_len = extractor.text_list().len();
        println!("{:?}: {:?}", file_name, text_len);
    }
}

fn to_relative_path(
    absolut_path: &Path,
    root_cargo_config_path: &Path,
) -> Result<PathBuf, MyError> {
    let cargo_config_path_abs = root_cargo_config_path.canonicalize()?;
    let project_root_path = cargo_config_path_abs.parent().ok_or(MyError::PathError)?;
    let relative_path = absolut_path
        .strip_prefix(project_root_path)
        .map_err(|_| MyError::PathError)?
        .to_str()
        .ok_or(MyError::PathError)?;
    Ok(Path::new("../..").join(relative_path))
}
