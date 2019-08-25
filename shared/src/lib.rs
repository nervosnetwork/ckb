//! # The Chain Library
//!
//! This Library contains the `ChainProvider` traits and `Chain` implement:
//!
//! - [ChainProvider](chain::chain::ChainProvider) provide index
//!   and store interface.
//! - [Chain](chain::chain::Chain) represent a struct which
//!   implement `ChainProvider`

pub mod error;
pub mod shared;
mod snapshot;
pub mod tx_pool;
mod tx_pool_ext;

pub use crate::snapshot::{Snapshot, SnapshotMgr};

pub(crate) const LOG_TARGET_TX_POOL: &str = "ckb-tx-pool";
pub(crate) const LOG_TARGET_CHAIN: &str = "ckb-chain";
