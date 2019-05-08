//! # The Chain Library
//!
//! This Library contains the `ChainProvider` traits and `Chain` implement:
//!
//! - [ChainProvider](chain::chain::ChainProvider) provide index
//!   and store interface.
//! - [Chain](chain::chain::Chain) represent a struct which
//!   implement `ChainProvider`

pub mod cell_set;
pub mod chain_state;
pub mod error;
pub mod shared;
pub mod tx_pool;
pub mod tx_pool_executor;
mod tx_proposal_table;

#[cfg(test)]
mod tests;
