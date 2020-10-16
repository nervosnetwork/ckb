//! The essential rust types for CKB.
//!
//! [Packed bytes] are not enough for all usage scenarios.
//!
//! This module provides essential rust types.
//!
//! Most of them is composed of [those packed bytes] or can convert between self and [those bytes].
//!
//! [Packed bytes]: ../packed/index.html
//! [those packed bytes]: ../packed/index.html
//! [those bytes]: ../packed/index.html

pub mod cell;
pub mod error;
pub mod service;

mod advanced_builders;
mod blockchain;
mod extras;
mod reward;
mod transaction_meta;
mod views;
pub use advanced_builders::{BlockBuilder, HeaderBuilder, TransactionBuilder};
pub use blockchain::{DepType, ScriptHashType};
pub use extras::{BlockExt, EpochExt, EpochNumberWithFraction, TransactionInfo};
pub use reward::{BlockEconomicState, BlockIssuance, BlockReward, MinerReward};
pub use transaction_meta::{TransactionMeta, TransactionMetaBuilder};
pub use views::{BlockView, HeaderView, TransactionView, UncleBlockVecView, UncleBlockView};

pub use ckb_occupied_capacity::{capacity_bytes, Capacity, Ratio, Result as CapacityResult};

/// Public key. It's a 512 bits fixed binary data.
pub type PublicKey = ckb_fixed_hash::H512;

/// Block number.
pub type BlockNumber = u64;

/// Epoch number.
pub type EpochNumber = u64;

/// Cycle number.
pub type Cycle = u64;

/// Version number.
pub type Version = u32;
