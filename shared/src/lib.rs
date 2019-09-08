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

pub use ckb_snapshot::Snapshot;

pub(crate) const LOG_TARGET_CHAIN: &str = "ckb-chain";
