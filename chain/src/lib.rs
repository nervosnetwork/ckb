//! # The Chain Library
//!
//! This Library contains the `Chain Service` implement:
//!
//! - [Chain](chain::chain::Chain) represent a struct which

mod cell;
pub mod chain;
pub mod prune;
pub mod switch;
#[cfg(test)]
mod tests;
