extern crate bigint;
extern crate bincode;
#[macro_use]
extern crate log;
extern crate avl_merkle as avl;
extern crate ckb_core as core;
extern crate ckb_db as db;
extern crate ckb_time as time;
extern crate ckb_util as util;
extern crate fnv;
extern crate lru_cache;
#[cfg(test)]
extern crate rand;
extern crate serde;
#[cfg(test)]
extern crate tempdir;
#[macro_use]
extern crate serde_derive;
extern crate ckb_notify;

pub mod cachedb;
pub mod chain;
pub mod consensus;
// mod config;
mod flat_serializer;
pub mod index;
pub mod store;

use db::batch::Col;

pub const COLUMNS: u32 = 9;
pub const COLUMN_INDEX: Col = Some(0);
pub const COLUMN_BLOCK_HEADER: Col = Some(1);
pub const COLUMN_BLOCK_BODY: Col = Some(2);
pub const COLUMN_META: Col = Some(3);
pub const COLUMN_TRANSACTION_ADDR: Col = Some(4);
pub const COLUMN_TRANSACTION_META: Col = Some(5);
pub const COLUMN_EXT: Col = Some(6);
pub const COLUMN_OUTPUT_ROOT: Col = Some(7);
pub const COLUMN_BLOCK_TRANSACTION_ADDRESSES: Col = Some(8);
