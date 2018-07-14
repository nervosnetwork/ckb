#![feature(int_to_from_bytes)]
extern crate bigint;
extern crate crossbeam_channel;
extern crate ethash;
#[macro_use]
extern crate log;
extern crate nervos_chain as chain;
extern crate nervos_core as core;
extern crate nervos_network as network;
extern crate nervos_notify;
extern crate nervos_pool as pool;
extern crate nervos_protocol;
extern crate nervos_sync as sync;
extern crate nervos_time as time;
extern crate rand;
#[macro_use]
extern crate serde_derive;

use bigint::H256;

pub mod miner;
mod sealer;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    // Max number of transactions this miner will assemble in a block
    pub max_tx: usize,
    pub ethash_path: Option<String>,
    pub redeem_script_hash: H256,
}
