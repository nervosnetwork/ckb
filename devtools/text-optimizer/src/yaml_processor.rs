use super::{error::MyError, types::TextInfo};
use std::io::Read;
use std::io::Write;
use std::{fs::File, path::PathBuf};

pub fn save_yaml(file: &PathBuf, data: &[TextInfo]) -> Result<(), MyError> {
    let mut file = File::create(file)?;
    file.write_fmt(format_args!(
        "# Number of TextInfo items: {}\n\n",
        data.len()
    ))?;
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
