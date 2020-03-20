//! Rust types.
//!
//! Packed bytes wrappers are not enough for all usage scenarios.

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
pub type PublicKey = ckb_fixed_hash::H512;
pub type BlockNumber = u64;
pub type EpochNumber = u64;
pub type Cycle = u64;
pub type Version = u32;
