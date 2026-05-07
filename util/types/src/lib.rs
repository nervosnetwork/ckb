//! # The Core Types Library
//!
//! This Library provides the essential types for CKB.

pub mod prelude;

pub use block_number_and_hash::BlockNumberAndHash;
pub use bytes;
pub use ckb_fixed_hash::{H160, H256, h160, h256};
pub use ckb_gen_types::packed;
pub use molecule::{self, error};
pub use numext_fixed_uint::{U128, U256, u256};
pub mod core;
pub mod global;

pub mod constants;
mod conversion;
mod extension;
pub mod utilities;

mod block_number_and_hash;
#[cfg(test)]
mod tests;
