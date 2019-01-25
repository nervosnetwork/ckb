//! # The DB Library
//!
//! This Library contains the `KeyValueDB` traits
//! which provides key-value store interface

pub mod config;
pub mod batch;
pub mod diskdb;
pub mod kvdb;
pub mod memorydb;

pub use crate::kvdb::KeyValueDB;
pub use crate::memorydb::MemoryKeyValueDB;
pub use crate::diskdb::RocksDB;
pub use crate::config::{DBConfig, RocksDBConfig};
