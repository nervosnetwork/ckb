//！The error type for Indexer.
use thiserror::Error;

/// A list specifying general categories of Indexer error.
#[derive(Error, Debug)]
pub enum Error {
    /// Underlying DB error
    #[error("Db error {0}")]
    DB(String),
    /// Invalid params error
    #[error("Invalid params {0}")]
    Params(String),
    /// Iterator limit exceeded
    #[error("Iteration limit exceeded {0}, you need to increase `iterator_next_limit` in the ckb.toml or tuning the query performance.")]
    IterLimitExceeded(usize),
}

impl Error {
    /// Creates a new Indexer Params error from an string payload.
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
