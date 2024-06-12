#[derive(Debug)]
pub enum MyError {
    CommitId,
    Io(std::io::Error),
    Serde(serde_yaml::Error),
    PathError,
}

impl From<std::io::Error> for MyError {
    fn from(error: std::io::Error) -> Self {
        MyError::Io(error)
    }
}

impl From<serde_yaml::Error> for MyError {
    fn from(error: serde_yaml::Error) -> Self {
        MyError::Serde(error)
    }
}
