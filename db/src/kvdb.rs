use crate::batch::{Batch, Col};
use bincode::Error as BcError;
use failure::Fail;
use rocksdb::{DBIterator as RdbIterator, Error as RdbError};
use std::error::Error as StdError;
use std::ops::Range;
use std::result;

pub type Error = ErrorKind;
pub type Result<T> = result::Result<T, Error>;
pub type DBIterator<'a> = RdbIterator<'a>;

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
    type Batch: DbBatch;
    fn db_batch(&self) -> Result<Self::Batch>;
    fn write(&self, batch: Batch) -> Result<()>;
    fn read(&self, col: Col, key: &[u8]) -> Result<Option<Vec<u8>>>;
    fn partial_read(&self, col: Col, key: &[u8], range: &Range<usize>) -> Result<Option<Vec<u8>>>;
    fn batch(&self) -> Batch {
        Batch::new()
    }
    /// returns an iterator over a column, starts from a key in forward direction.
    /// TODO use Rocksdb's DBIterator as a temp soluction, refactor it to associated type, same as Batch
    fn iter(&self, col: Col, key: &[u8]) -> Option<DBIterator> {
        None
    }
}

pub trait DbBatch {
    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<()>;
    fn delete(&mut self, key: &[u8]) -> Result<()>;
    fn commit(self) -> Result<()>;
}
