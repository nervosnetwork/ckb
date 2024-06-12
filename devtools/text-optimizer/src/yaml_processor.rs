use super::{
    error::MyError,
    types::{Category, Meta, TextInfo},
    GITHUB_REPO,
};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::str::FromStr;
use std::{fs::File, path::PathBuf};
use std::{io::Read, path::Path};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub struct TextInfoSave {
    original: String,
    editable: String,
    metadata: Metadata,
}

impl TextInfoSave {
    pub fn from_text_info(text_info: TextInfo, git_repo: &str, commit_id: &str) -> Self {
        let metadata = Metadata::from_meta(
            text_info.metadata(),
            git_repo,
            commit_id,
            text_info.metadata().file(),
        );

        TextInfoSave {
            original: text_info.original().to_owned(),
            editable: text_info.editable().to_owned(),
            metadata,
        }
    }
}

impl From<TextInfoSave> for TextInfo {
    fn from(text_info_save: TextInfoSave) -> Self {
        let metadata = Meta::from(text_info_save.metadata);
        TextInfo::new(text_info_save.original, text_info_save.editable, metadata)
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub struct Metadata {
    category: Category,
    file: PathBuf,
    code_line_link: Vec<String>,
}

impl From<Metadata> for Meta {
    fn from(metadata: Metadata) -> Self {
        let code_lines: Vec<usize> = metadata
            .code_line_link
            .iter()
            .map(|link| {
                let line = link.split("#L").last().expect("split line");
                usize::from_str(line).expect("parse line")
            })
            .collect();
        Meta::new(metadata.category, metadata.file, code_lines)
    }
}

impl Metadata {
    pub fn from_meta(meta: &Meta, github_repo: &str, commit_id: &str, file: &Path) -> Self {
        let file = file.strip_prefix("../..").expect("strip prefix");
        let github_link_prefix = format!("{}/{}/{}", github_repo, commit_id, file.display());
        let code_line_link: Vec<String> = meta
            .start_lines()
            .iter()
            .map(|line| format!("{}#L{}", github_link_prefix, line))
            .collect();

        Metadata {
            category: meta.category().to_owned(),
            file: meta.file().to_owned(),
            code_line_link,
        }
    }
}

pub fn save_yaml(file: &PathBuf, data: &[TextInfo], commit_id: &str) -> Result<(), MyError> {
    let mut file = File::create(file)?;

    // Convert TextInfo to TextInfoSave
    let data_save: Vec<TextInfoSave> = data
        .iter()
        .map(|text_info| TextInfoSave::from_text_info(text_info.clone(), GITHUB_REPO, commit_id))
        .collect();

    file.write_fmt(format_args!(
        "# Number of TextInfo items: {}\n\n",
        data.len()
    ))?;
    serde_yaml::to_writer(file, &data_save)?;
    Ok(())
}

pub fn load_yaml(filename: &PathBuf) -> Result<Vec<TextInfo>, MyError> {
    let mut file = File::open(filename)?;
    let mut yaml_content = String::new();
    file.read_to_string(&mut yaml_content)?;

    let extracted_texts: Vec<TextInfoSave> = serde_yaml::from_str(&yaml_content)?;
    let extracted_texts = extracted_texts
        .iter()
        .map(|text_info_save| TextInfo::from(text_info_save.clone()))
        .collect();
    Ok(extracted_texts)
}
