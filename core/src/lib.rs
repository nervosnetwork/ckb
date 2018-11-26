//! # The Core Type Library
//!
//! This Library provides the essential types for building ckb.

extern crate bigint;
extern crate bincode;
extern crate ckb_util;
extern crate crypto;
extern crate hash;
#[macro_use]
extern crate serde_derive;
extern crate bit_vec;
extern crate crossbeam_channel as channel;
extern crate fnv;
extern crate merkle_root;

pub mod block;
pub mod cell;
pub mod chain;
pub mod difficulty;
pub mod error;
pub mod extras;
pub mod header;
pub mod script;
pub mod service;
pub mod transaction;
pub mod transaction_meta;
pub mod uncle;

pub use error::Error;

pub type PublicKey = bigint::H512;
pub type BlockNumber = u64;
pub type Capacity = u64;
