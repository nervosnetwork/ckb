use adapter::{ChainToNetAndPoolAdapter, NetToChainAndPoolAdapter};
use bigint;
use chain;
use chain::chain::Chain;
use config::Config;
use ctrlc;
use db::cachedb::CacheKeyValueDB;
use db::diskdb::RocksKeyValueDB;
use db::store::ChainKVStore;
use logger;
use miner::miner::Miner;
use network::Network;
use pool::TransactionPool;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use util::{Condvar, Mutex};

pub fn run(config: Config) {
    logger::init(config.logger_config()).expect("Init Logger");

    info!(target: "main", "Value for config: {:?}", config);

    let db = CacheKeyValueDB::new(RocksKeyValueDB::open(Path::new(&config.db_path)));
    let store = ChainKVStore { db: Box::new(db) };

    let tx_pool = Arc::new(TransactionPool::default());

    let chain_adapter = Arc::new(ChainToNetAndPoolAdapter::new(tx_pool.clone()));
    let chain = Arc::new(
        Chain::init(store, chain_adapter.clone(), &chain::genesis::genesis_dev()).unwrap(),
    );

    let kg = Arc::new(config.key_group());

    let net_adapter = NetToChainAndPoolAdapter::new(kg, &chain, tx_pool.clone());

    let network = Arc::new(Network::init(net_adapter, config.network).unwrap());

    chain_adapter.init(&network);

    let miner = Miner {
        chain,
        tx_pool,
        miner_key: config.signer.miner_private_key,
        signer_key: bigint::H256::from(&config.signer.signer_private_key[..]),
    };

    let _ = thread::Builder::new()
        .name("network".to_string())
        .spawn(move || {
            network.start();
        });

    let _ = thread::Builder::new()
        .name("miner".to_string())
        .spawn(move || {
            miner.run_loop();
        });

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");

    logger::flush();
}

fn wait_for_exit() {
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
