//! # The Chain Library
//!
//! This Library contains the `ChainProvider` traits and `Chain` implement:
//!
//! - [ChainProvider](chain::chain::ChainProvider) provide index
//!   and store interface.
//! - [Chain](chain::chain::Chain) represent a struct which
//!   implement `ChainProvider`

extern crate bigint;
extern crate ckb_chain_spec;
extern crate ckb_core;
extern crate ckb_db;
extern crate ckb_notify;
extern crate ckb_shared;
extern crate ckb_time;
extern crate ckb_verification;
#[macro_use]
extern crate log;
#[macro_use]
extern crate crossbeam_channel as channel;

#[cfg(test)]
extern crate rand;
#[cfg(test)]
extern crate tempfile;

pub mod chain;
pub mod error;
