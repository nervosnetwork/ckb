//! # The Chain Library
//!
//! This Library contains the `ChainProvider` traits and `Chain` implement:
//!
//! - [ChainProvider](chain::chain::ChainProvider) provide index
//!   and store interface.
//! - [Chain](chain::chain::Chain) represent a struct which
//!   implement `ChainProvider`

pub mod chain;

#[cfg(test)]
mod tests;
