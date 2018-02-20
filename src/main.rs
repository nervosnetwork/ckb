#[macro_use]
extern crate clap;
#[macro_use]
extern crate log;
extern crate nervos_chain as chain;
extern crate nervos_db as db;
extern crate nervos_miner as miner;
extern crate nervos_network as network;
extern crate nervos_pool as pool;
extern crate nervos_util as util;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate toml;

mod config;

use chain::adapter::ChainToNetAndPoolAdapter;
use chain::chain::Chain;
use chain::store::ChainKVStore;
use config::Config;
use db::kvdb::MemoryKeyValueDB;
use miner::miner::Miner;
use network::Network;
use pool::{OrphanBlockPool, TransactionPool};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use util::logger;
use util::wait_for_exit;

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

    let network = Network {};
    let orphan_pool = OrphanBlockPool {};
    let chain_adapter = ChainToNetAndPoolAdapter {
        orphan_pool: Box::new(orphan_pool),
        network: Box::new(network),
    };

    let chain = Chain::init(
        Arc::new(store),
        Arc::new(chain_adapter),
        &chain::genesis::genesis_dev(),
    ).unwrap();

    let tx_pool = TransactionPool {
        pool: RwLock::new(HashMap::new()),
    };
    let miner = Miner {
        chain: Box::new(chain),
        tx_pool: Box::new(tx_pool),
        private_key: vec![0, 1, 2],
    };

    miner.run_loop();

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");

    logger::flush();
}
