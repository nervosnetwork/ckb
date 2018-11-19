//! # The DB Library
//!
//! This Library contains the `KeyValueDB` traits
//! which provides key-value store interface

extern crate bincode;
extern crate ckb_util as util;
extern crate fnv;
extern crate rocksdb;

pub mod batch;
pub mod diskdb;
pub mod kvdb;
pub mod memorydb;

#[cfg(test)]
extern crate tempdir;
