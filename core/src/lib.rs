//! # The Core Type Library
//!
//! This Library provides the essential types for building ckb.

pub mod alert;
pub mod block;
pub mod cell;
pub mod difficulty;
pub mod extras;
pub mod header;
pub mod script;
pub mod service;
pub mod transaction;
pub mod transaction_meta;
pub mod uncle;

pub use bytes::Bytes;
pub use ckb_occupied_capacity::{capacity_bytes, Capacity};
pub type PublicKey = numext_fixed_hash::H512;
pub type BlockNumber = u64;
pub type EpochNumber = u64;
pub type Cycle = u64;
pub type Version = u32;
