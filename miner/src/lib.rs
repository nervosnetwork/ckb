extern crate bigint;
extern crate ckb_chain;
extern crate ckb_core;
extern crate ckb_network;
extern crate ckb_notify;
extern crate ckb_protocol;
extern crate ckb_rpc;
extern crate ckb_shared;
#[macro_use]
extern crate crossbeam_channel as channel;
#[macro_use]
extern crate log;
extern crate ckb_sync;
extern crate flatbuffers;
extern crate rand;
#[macro_use]
extern crate serde_derive;
extern crate ckb_pow;

mod config;
mod miner;

pub use config::Config;
pub use miner::MinerService;
