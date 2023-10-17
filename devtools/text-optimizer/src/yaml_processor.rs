use super::{
    error::MyError,
    types::{Category, Meta, TextInfo},
    GITHUB_REPO,
};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::io::Write;
use std::{fs::File, path::PathBuf};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub struct TextInfoSave {
    original: String,
    editable: String,
    metadata: Metadata,
}

impl TextInfoSave {
    pub fn from_text_info(text_info: TextInfo, git_repo: &str, commit_id: &str) -> Self {
        // 使用 Metadata 的 from_meta 方法进行 Meta 到 Metadata 的转换
        let metadata = Metadata::from_meta(text_info.metadata(), git_repo, commit_id);

        // 创建 TextInfoSave 结构体并返回
        TextInfoSave {
            original: text_info.original().to_owned(),
            editable: text_info.editable().to_owned(),
            metadata,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub struct Metadata {
    category: Category,
    file: PathBuf,
    code_line_link: Vec<String>,
}

impl Metadata {
    // 定义从 Meta 到 Metadata 的转换方法
    pub fn from_meta(meta: &Meta, github_repo: &str, commit_id: &str) -> Self {
        // 创建 GitHub 代码行链接的前缀
        let github_link_prefix = format!("{}/{}/", github_repo, commit_id);

        // 为每个代码行生成 GitHub 链接
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

    let extracted_texts: Vec<TextInfo> = serde_yaml::from_str(&yaml_content)?;

    Ok(extracted_texts)
}
