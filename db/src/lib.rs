//! # The DB Library
//!
//! This Library contains the `KeyValueDB` traits
//! which provides key-value store interface

use failure::Fail;
use std::ops::Range;
use std::result;

pub mod config;
pub mod memorydb;
pub mod rocksdb;

pub use crate::config::DBConfig;
pub use crate::memorydb::MemoryKeyValueDB;
pub use crate::rocksdb::RocksDB;

pub type Col = u32;
pub type Result<T> = result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq, Fail)]
pub enum Error {
    #[fail(display = "DBError {}", _0)]
    DBError(String),
}

pub trait KeyValueDB: Sync + Send {
    type Batch: DbBatch;
    fn read(&self, col: Col, key: &[u8]) -> Result<Option<Vec<u8>>>;
    fn partial_read(&self, col: Col, key: &[u8], range: &Range<usize>) -> Result<Option<Vec<u8>>>;
    fn batch(&self) -> Result<Self::Batch>;
    fn traverse<F>(&self, col: Col, callback: F) -> Result<()>
    where
        F: FnMut(&[u8], &[u8]) -> Result<()>;
}

pub trait DbBatch {
    fn insert(&mut self, col: Col, key: &[u8], value: &[u8]) -> Result<()>;
    fn delete(&mut self, col: Col, key: &[u8]) -> Result<()>;
    fn commit(self) -> Result<()>;
}
