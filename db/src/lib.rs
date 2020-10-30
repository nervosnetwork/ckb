//! # The DB Library
//!
//! This Library contains the `KeyValueDB` traits
//! which provides key-value store interface

use ckb_error::{Error, InternalErrorKind};
use std::fmt::{Debug, Display};
use std::result;

pub mod db;
pub mod iter;
pub mod snapshot;
pub mod transaction;
pub mod write_batch;

pub use crate::db::RocksDB;
pub use crate::iter::DBIterator;
pub use crate::snapshot::RocksDBSnapshot;
pub use crate::transaction::{RocksDBTransaction, RocksDBTransactionSnapshot};
pub use crate::write_batch::RocksDBWriteBatch;
pub use rocksdb::{
    self as internal, DBPinnableSlice, DBVector, Direction, Error as DBError, IteratorMode,
};

/// TODO(doc): @quake
pub type Col = &'static str;
/// TODO(doc): @quake
pub type Result<T> = result::Result<T, Error>;

fn internal_error<S: Display + Debug + Sync + Send + 'static>(reason: S) -> Error {
    InternalErrorKind::Database.reason(reason).into()
}
