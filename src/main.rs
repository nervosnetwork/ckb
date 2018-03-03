extern crate bigint;
#[macro_use]
extern crate clap;
extern crate ctrlc;
#[macro_use]
extern crate log;
extern crate logger;
extern crate nervos_chain as chain;
extern crate nervos_core as core;
extern crate nervos_db as db;
extern crate nervos_miner as miner;
extern crate nervos_network as network;
extern crate nervos_pool as pool;
extern crate nervos_time as time;
extern crate nervos_util as util;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate toml;

mod config;
mod adapter;

use adapter::{ChainToNetAndPoolAdapter, NetToChainAndPoolAdapter};
use chain::chain::Chain;
use chain::store::ChainKVStore;
use config::Config;
use db::kvdb::MemoryKeyValueDB;
use miner::miner::Miner;
use network::Network;
use pool::TransactionPool;
use std::sync::Arc;
use util::{Condvar, Mutex};

fn main() {
    let matches = clap_app!(nervos =>
        (version: "0.1")
        (author: "Nervos <dev@nervos.org>")
        (about: "Nervos")
        (@arg CONFIG: -c --config +takes_value "Sets a custom config file")
    ).get_matches();

    let config_path = matches.value_of("config").unwrap_or("default.toml");
    let config = Config::load(config_path);

    logger::init(config.logger_config()).expect("Init Logger");

    info!(target: "main", "Value for config: {:?}", config);

    let db = MemoryKeyValueDB::default();
    let store = ChainKVStore { db: Box::new(db) };

    let tx_pool = Arc::new(TransactionPool::default());

    let chain_adapter = Arc::new(ChainToNetAndPoolAdapter::new(tx_pool.clone()));
    let chain = Arc::new(
        Chain::init(
            Arc::new(store),
            chain_adapter.clone(),
            &chain::genesis::genesis_dev(),
        ).unwrap(),
    );

    let kg = Arc::new(config.key_group());

    let net_adapter = NetToChainAndPoolAdapter::new(kg, chain.clone(), tx_pool.clone());

    let network = Arc::new(Network {
        adapter: net_adapter,
    });

    chain_adapter.init(network);

    // chain_adapter.network = network;

    let miner = Miner {
        chain: chain,
        tx_pool: tx_pool,
        miner_key: config.miner_private_key,
        signer_key: config.signer_private_key,
    };

    miner.run_loop();

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");

    logger::flush();
}

pub fn wait_for_exit() {
    let exit = Arc::new((Mutex::new(()), Condvar::new()));

    // Handle possible exits
    let e = exit.clone();
    let _ = ctrlc::set_handler(move || {
        e.1.notify_all();
    });

    // Wait for signal
    let mut l = exit.0.lock();
    exit.1.wait(&mut l);
}
