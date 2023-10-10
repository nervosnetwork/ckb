use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TextInfo {
    original: String,
    editable: String,
    metadata: Meta,
}

impl TextInfo {
    pub fn new(original: String, metadata: Meta) -> Self {
        TextInfo {
            original: original.to_owned(),
            editable: original,
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

    pub fn metadata(&self) -> &Meta {
        &self.metadata
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Meta {
    category: Category,
    file: PathBuf,
    start_line: usize,
    end_line: usize,
}

impl Meta {
    pub fn new(category: Category, file: PathBuf, start_line: usize, end_line: usize) -> Self {
        Meta {
            category,
            file,
            start_line,
            end_line,
        }
    }

    #[allow(dead_code)]
    pub fn category(&self) -> &Category {
        &self.category
    }

    pub fn file(&self) -> &PathBuf {
        &self.file
    }

    pub fn start_line(&self) -> usize {
        self.start_line
    }

    #[allow(dead_code)]
    pub fn end_line(&self) -> usize {
        self.end_line
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
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
