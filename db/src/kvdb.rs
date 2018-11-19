use batch::{Batch, Key, Value};
use bincode::Error as BcError;
use rocksdb::Error as RdbError;
use std::error::Error as StdError;
use std::result;

type Error = Box<ErrorKind>;
pub type Result<T> = result::Result<T, Error>;

#[derive(Debug)]
pub enum ErrorKind {
    DBError(String),
    SerializationError(String),
}

impl From<BcError> for Error {
    fn from(err: BcError) -> Error {
        Box::new(ErrorKind::SerializationError(err.description().to_string()))
    }
}

impl From<RdbError> for Error {
    fn from(err: RdbError) -> Error {
        Box::new(ErrorKind::DBError(err.into()))
    }
}

pub trait KeyValueDB: Sync + Send {
    fn write(&self, batch: Batch) -> Result<()>;
    fn read(&self, key: &Key) -> Result<Option<Value>>;
}
