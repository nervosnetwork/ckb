//! Multi-signatures.
//!
//! A m-of-n signature mechanism requires m valid signatures signed by m different keys from
//! the pre-configured n keys.
#[macro_use]
extern crate failure;

pub mod error;
pub mod secp256k1;
