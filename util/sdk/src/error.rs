use std::io;

use failure::Fail;

use crate::index::IndexError;
use crate::wallet::KeyStoreError;

#[derive(Debug, Fail)]
pub enum Error {
    #[fail(display = "Rocksdb error: {}", _0)]
    Rocksdb(rocksdb::Error),
    #[fail(display = "IO error: {}", _0)]
    Io(io::Error),
    #[fail(display = "KeyStore error: {}", _0)]
    KeyStore(KeyStoreError),
    #[fail(display = "Index DB error: {}", _0)]
    Index(IndexError),
    #[fail(display = "Other error: {}", _0)]
    Other(String),
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

impl From<rocksdb::Error> for Error {
    fn from(err: rocksdb::Error) -> Error {
        Error::Rocksdb(err)
    }
}

impl From<IndexError> for Error {
    fn from(err: IndexError) -> Error {
        Error::Index(err)
    }
}

impl From<String> for Error {
    fn from(err: String) -> Error {
        Error::Other(err)
    }
}
