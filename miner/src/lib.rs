extern crate bigint;
extern crate ckb_chain as chain;
extern crate ckb_core as core;
extern crate ckb_network as network;
extern crate ckb_notify;
extern crate ckb_protocol;
extern crate ckb_rpc as rpc;
extern crate ckb_shared as shared;
#[macro_use]
extern crate crossbeam_channel as channel;
#[macro_use]
extern crate log;
extern crate ckb_sync as sync;
extern crate flatbuffers;
extern crate rand;
#[macro_use]
extern crate serde_derive;
extern crate ckb_pow;

#[cfg(test)]
extern crate ckb_db as db;
#[cfg(test)]
extern crate ckb_pool as pool;
#[cfg(test)]
extern crate ckb_verification as verification;

mod config;
mod miner;

pub use config::Config;
pub use miner::MinerService;
