//! # The DB Library
//!
//! This Library contains the `KeyValueDB` traits
//! which provides key-value store interface

use ckb_error::{Error, InternalErrorKind};
use std::fmt::{Debug, Display};
use std::result;

pub mod config;
pub mod db;
pub mod iter;
mod migration;
pub mod snapshot;
pub mod transaction;

pub use crate::config::DBConfig;
pub use crate::db::RocksDB;
pub use crate::iter::DBIterator;
pub use crate::migration::{DefaultMigration, Migration, Migrations};
pub use crate::snapshot::RocksDBSnapshot;
pub use crate::transaction::{RocksDBTransaction, RocksDBTransactionSnapshot};
pub use rocksdb::{
    ColumnFamily, DBPinnableSlice, DBVector, Direction, Error as DBError, IteratorMode,
    ReadOptions, WriteBatch,
};

pub type Col = &'static str;
pub type Result<T> = result::Result<T, Error>;

fn internal_error<S: Display + Debug + Sync + Send + 'static>(reason: S) -> Error {
    InternalErrorKind::Database.reason(reason).into()
}
