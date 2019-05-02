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
mod tx_proposal_table;

// These tests are from testnet data dump, they are hard to maintenance.
// Although they pass curruent unit tests, we still comment out here,
// Keep the code for future unit test refactoring reference.
// #[cfg(test)]
// mod tests;
