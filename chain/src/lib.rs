//! # The Chain Library
//!
//! This Library contains the `ChainProvider` traits and `Chain` implement:
//!
//! - [ChainProvider](chain::chain::ChainProvider) provide index
//!   and store interface.
//! - [Chain](chain::chain::Chain) represent a struct which
//!   implement `ChainProvider`

extern crate avl_merkle as avl;
extern crate bigint;
extern crate bincode;
extern crate byteorder;
extern crate ckb_core as core;
extern crate ckb_db as db;
extern crate ckb_notify;
extern crate ckb_time as time;
extern crate ckb_util as util;
extern crate ethash;
extern crate fnv;
extern crate hash;
#[macro_use]
extern crate log;
extern crate lru_cache;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate crossbeam_channel;
#[cfg(test)]
extern crate rand;
#[cfg(test)]
extern crate tempdir;

pub mod cachedb;
pub mod chain;
pub mod consensus;
// mod config;
pub mod error;
mod flat_serializer;
pub mod index;
pub mod pow;
pub mod store;

use db::batch::Col;

// REMEMBER to update the const defined in util/avl/src/lib.rs as well
pub const COLUMNS: u32 = 12;
pub const COLUMN_INDEX: Col = Some(0);
pub const COLUMN_BLOCK_HEADER: Col = Some(1);
pub const COLUMN_BLOCK_BODY: Col = Some(2);
pub const COLUMN_BLOCK_UNCLE: Col = Some(3);
pub const COLUMN_META: Col = Some(4);
pub const COLUMN_TRANSACTION_ADDR: Col = Some(5);
pub const COLUMN_TRANSACTION_META: Col = Some(6);
pub const COLUMN_EXT: Col = Some(7);
pub const COLUMN_OUTPUT_ROOT: Col = Some(8);
pub const COLUMN_BLOCK_TRANSACTION_ADDRESSES: Col = Some(9);
pub const COLUMN_BLOCK_TRANSACTION_IDS: Col = Some(10);
pub const COLUMN_BLOCK_PROPOSAL_IDS: Col = Some(11);

pub use pow::{Clicker, CuckooEngine, DummyPowEngine, EthashEngine, PowEngine};
