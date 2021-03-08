//! CKB verification contextual
//!
//! This crate implements CKB contextual verification by newtypes abstraction struct
mod contextual_block_verifier;
#[cfg(test)]
mod tests;
mod uncles_verifier;

pub use crate::contextual_block_verifier::{ContextualBlockVerifier, VerifyContext};
const LOG_TARGET: &str = "ckb_chain";
