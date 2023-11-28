use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;
use std::vec;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub struct TextInfo {
    original: String,
    editable: String,
    metadata: Meta,
}

impl TextInfo {
    pub fn new(original: String, editable: String, metadata: Meta) -> Self {
        TextInfo {
            original,
            editable,
            metadata,
        }
    }

    #[allow(dead_code)]
    pub fn original(&self) -> &str {
        &self.original
    }

    #[allow(dead_code)]
    pub fn editable(&self) -> &str {
        &self.editable
    }

    #[allow(dead_code)]
    pub fn metadata(&self) -> &Meta {
        &self.metadata
    }

    pub fn append_new_line(&mut self, new: usize) {
        self.metadata.append_new_line(new)
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub struct Meta {
    category: Category,
    file: PathBuf,
    code_lines: Vec<usize>,
}

impl Meta {
    pub fn new_line(category: Category, file: PathBuf, start_line: usize) -> Self {
        Meta {
            category,
            file,
            code_lines: vec![start_line],
        }
    }

    pub fn new(category: Category, file: PathBuf, code_lines: Vec<usize>) -> Self {
        Meta {
            category,
            file,
            code_lines,
        }
    }

    #[allow(dead_code)]
    pub fn category(&self) -> &Category {
        &self.category
    }

    #[allow(dead_code)]
    pub fn file(&self) -> &PathBuf {
        &self.file
    }

    #[allow(dead_code)]
    pub fn start_lines(&self) -> &[usize] {
        &self.code_lines
    }

    pub fn append_new_line(&mut self, new: usize) {
        self.code_lines.push(new)
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub enum Category {
    ClapHelp,
    ClapAbout,

    LogDebug,
    LogWarn,
    LogInfo,
    LogError,
    LogTrace,

    StdOutput,
    StdError,

    ThisError,
}

impl FromStr for Category {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "help" => Ok(Category::ClapHelp),
            "about" => Ok(Category::ClapAbout),
            "debug" => Ok(Category::LogDebug),
            "warn" => Ok(Category::LogWarn),
            "info" => Ok(Category::LogInfo),
            "error" => Ok(Category::LogError),
            "trace" => Ok(Category::LogTrace),
            "println" => Ok(Category::StdOutput),
            "eprintln" => Ok(Category::StdError),
            _ => Err(()),
        }
    }
}
