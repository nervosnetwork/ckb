use chain;
use chain::chain::Chain;
use chain::store::ChainKVStore;
use config::Config;
use ctrlc;
use db::cachedb::CacheKeyValueDB;
use db::diskdb::RocksKeyValueDB;
use ethash::Ethash;
use logger;
use miner::miner::Miner;
use nervos_notify::Notify;
use network::Network;
use pool::*;
use rpc::RpcServer;
use std::sync::Arc;
use std::thread;
use sync::node::Node;
use util::{Condvar, Mutex};

pub fn run(config: Config) {
    logger::init(config.logger_config()).expect("Init Logger");

    info!(target: "main", "Value for config: {:?}", config);

    // let lock = Arc::new(Mutex::new(()));

    let db = CacheKeyValueDB::new(RocksKeyValueDB::open(&config.dirs.db));
    let store = ChainKVStore { db };

    let ethash_path = config.dirs.base.join(".ethash");
    let _ = ::std::fs::create_dir_all(&ethash_path);
    let ethash = Arc::new(Ethash::new(ethash_path));

    // let pool_net_adapter = Arc::new(PoolToNetAdapter::new());
    // let pool_chain_adapter = Arc::new(PoolToChainAdapter::new());

    let notify = Notify::new();
    let chain = Arc::new(Chain::init(store, &chain::genesis::genesis_dev()).unwrap());

    let tx_pool = Arc::new(TransactionPool::new(
        PoolConfig::default(),
        Arc::clone(&chain),
        notify.clone(),
    ));

    let network = Arc::new(Network::new(config.network));
    let node_network = Arc::clone(&network);
    let node_chain = Arc::clone(&chain);
    let node = Node::new(node_network, node_chain, &tx_pool, notify.clone());
    node.start();

    // chain_adapter.init(&network);
    // pool_net_adapter.init(&network);
    // pool_chain_adapter.init(&chain);

    let miner_chain = Arc::clone(&chain);
    let miner = Miner::new(miner_chain, &tx_pool, &network, &ethash, &notify);

    let _ = thread::Builder::new()
        .name("miner".to_string())
        .spawn(move || {
            miner.run_loop();
        });

    let rpc_server = RpcServer { config: config.rpc };
    let network_clone = Arc::clone(&network);

    let chain_clone = Arc::clone(&chain);
    let _ = thread::Builder::new()
        .name("rpc".to_string())
        .spawn(move || {
            rpc_server.start(network_clone, chain_clone);
        });

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");

    // network.flush();
    logger::flush();
}

fn wait_for_exit() {
    let exit = Arc::new((Mutex::new(()), Condvar::new()));

    // Handle possible exits
    let e = Arc::<(Mutex<()>, Condvar)>::clone(&exit);
    let _ = ctrlc::set_handler(move || {
        e.1.notify_all();
    });

    // Wait for signal
    let mut l = exit.0.lock();
    exit.1.wait(&mut l);
}
