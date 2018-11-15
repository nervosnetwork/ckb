//! # The Chain Library
//!
//! This Library contains the `ChainProvider` traits and `Chain` implement:
//!
//! - [ChainProvider](chain::chain::ChainProvider) provide index
//!   and store interface.
//! - [Chain](chain::chain::Chain) represent a struct which
//!   implement `ChainProvider`

extern crate bigint;
extern crate ckb_chain_spec as chain_spec;
extern crate ckb_core as core;
extern crate ckb_db as db;
extern crate ckb_notify;
extern crate ckb_shared as shared;
extern crate ckb_time as time;
extern crate ckb_verification as verification;
#[macro_use]
extern crate log;
#[macro_use]
extern crate crossbeam_channel as channel;
extern crate ckb_util as util;

#[cfg(test)]
extern crate rand;
#[cfg(test)]
extern crate tempfile;

pub mod chain;
pub mod error;
