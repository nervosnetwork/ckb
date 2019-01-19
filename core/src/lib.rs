//! # The Core Type Library
//!
//! This Library provides the essential types for building ckb.

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

pub use crate::error::Error;

pub type PublicKey = numext_fixed_hash::H512;
pub type BlockNumber = u64;
pub type Capacity = u64;
pub type Cycle = u64;

pub type TriggerForCi = u64;
