use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Db error {0}")]
    DB(String),
    #[error("Invalid params {0}")]
    Params(String),
}

impl Error {
    pub fn invalid_params<S>(s: S) -> Error
    where
        S: Into<String>,
    {
        Error::Params(s.into())
    }
}

impl From<rocksdb::Error> for Error {
    fn from(e: rocksdb::Error) -> Error {
        Error::DB(e.to_string())
    }
}
