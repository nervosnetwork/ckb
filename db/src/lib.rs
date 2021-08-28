//! # The DB Library
//!
//! This Library contains the `KeyValueDB` traits
//! which provides key-value store interface

use ckb_error::{Error, InternalErrorKind};
use std::{fmt, result};

pub mod db;
pub mod iter;
pub mod read_only_db;
pub mod snapshot;
pub mod transaction;
pub mod write_batch;

#[cfg(test)]
mod tests;

pub use crate::db::RocksDB;
pub use crate::iter::DBIterator;
pub use crate::read_only_db::ReadOnlyDB;
pub use crate::snapshot::RocksDBSnapshot;
pub use crate::transaction::{RocksDBTransaction, RocksDBTransactionSnapshot};
pub use crate::write_batch::RocksDBWriteBatch;
pub use rocksdb::{
    self as internal, DBPinnableSlice, DBVector, Direction, Error as DBError, IteratorMode,
    ReadOptions, WriteBatch,
};

/// The type returned by database methods.
pub type Result<T> = result::Result<T, Error>;

fn internal_error<S: fmt::Display>(reason: S) -> Error {
    let message = reason.to_string();
    if message.starts_with("Corruption:") {
        InternalErrorKind::Database.other(message).into()
    } else {
        InternalErrorKind::DataCorrupted.other(message).into()
    }
}
