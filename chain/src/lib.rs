extern crate bigint;
extern crate bincode;
#[macro_use]
extern crate log;
extern crate avl_merkle as avl;
extern crate ethash;
extern crate fnv;
extern crate lru_cache;
extern crate nervos_core as core;
extern crate nervos_db as db;
extern crate nervos_time as time;
extern crate nervos_util as util;
extern crate nervos_verification;
extern crate rand;
extern crate serde;
#[cfg(test)]
extern crate tempdir;

pub mod cachedb;
pub mod chain;
pub mod index;
mod spec;
pub mod store;
pub use spec::Spec;

use db::batch::Col;

pub const COLUMNS: u32 = 8;
pub const COLUMN_INDEX: Col = Some(0);
pub const COLUMN_BLOCK_HEADER: Col = Some(1);
pub const COLUMN_BLOCK_BODY: Col = Some(2);
pub const COLUMN_META: Col = Some(3);
pub const COLUMN_TRANSACTION_ADDR: Col = Some(4);
pub const COLUMN_TRANSACTION_META: Col = Some(5);
pub const COLUMN_EXT: Col = Some(6);
pub const COLUMN_OUTPUT_ROOT: Col = Some(7);
