use super::types::TextInfo;
use std::io::Read;
use std::{fs::File, path::PathBuf};

#[derive(Debug)]
pub enum MyError {
    IoError(std::io::Error),
    SerdeError(serde_yaml::Error),
}

impl From<std::io::Error> for MyError {
    fn from(error: std::io::Error) -> Self {
        MyError::IoError(error)
    }
}

impl From<serde_yaml::Error> for MyError {
    fn from(error: serde_yaml::Error) -> Self {
        MyError::SerdeError(error)
    }
}

pub fn save_yaml(file: &PathBuf, data: &[TextInfo]) -> Result<(), MyError> {
    let file = File::create(file)?;
    serde_yaml::to_writer(file, data)?;
    Ok(())
}

pub fn load_yaml(filename: &PathBuf) -> Result<Vec<TextInfo>, MyError> {
    let mut file = File::open(filename)?;
    let mut yaml_content = String::new();
    file.read_to_string(&mut yaml_content)?;

    let extracted_texts: Vec<TextInfo> = serde_yaml::from_str(&yaml_content)?;

    Ok(extracted_texts)
}
