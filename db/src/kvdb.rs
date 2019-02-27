use crate::batch::{Batch, Col};
use bincode::Error as BcError;
use failure::Fail;
use rocksdb::Error as RdbError;
use std::error::Error as StdError;
use std::ops::Range;
use std::result;

pub type Error = ErrorKind;
pub type Result<T> = result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq, Fail)]
pub enum ErrorKind {
    #[fail(display = "DBError {}", _0)]
    DBError(String),
    #[fail(display = "SerializationError {}", _0)]
    SerializationError(String),
}

impl From<BcError> for Error {
    fn from(err: BcError) -> Error {
        ErrorKind::SerializationError(err.description().to_string())
    }
}

impl From<RdbError> for Error {
    fn from(err: RdbError) -> Error {
        ErrorKind::DBError(err.into())
    }
}

pub trait KeyValueDB: Sync + Send {
    fn write(&self, batch: Batch) -> Result<()>;
    fn read(&self, col: Col, key: &[u8]) -> Result<Option<Vec<u8>>>;
    fn len(&self, col: Col, key: &[u8]) -> Result<Option<usize>>;
    fn partial_read(&self, col: Col, key: &[u8], range: &Range<usize>) -> Result<Option<Vec<u8>>>;
    fn cols(&self) -> u32;
    fn batch(&self) -> Batch {
        Batch::new()
    }
}
