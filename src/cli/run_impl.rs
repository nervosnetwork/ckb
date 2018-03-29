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
use rpc::RpcServer;
use std::sync::Arc;
use std::thread;
use sync::node::Node;
use util::{Condvar, Mutex};

pub fn run(config: Config) {
    logger::init(config.logger_config()).expect("Init Logger");

    info!(target: "main", "Value for config: {:?}", config);

    let lock = Arc::new(Mutex::new(()));

    let db = CacheKeyValueDB::new(RocksKeyValueDB::open(&config.dirs.db));
    let store = ChainKVStore { db };

    let tx_pool = Arc::new(TransactionPool::default());

    let chain = Arc::new(Chain::init(store, &chain::genesis::genesis_dev()).unwrap());

    // let kg = Arc::new(config.key_group());

    let network = Arc::new(Network::new(config.network));

    let node_network = Arc::clone(&network);
    let node_chain = Arc::clone(&chain);
    let node = Node::new(node_network, node_chain, &tx_pool, &lock);
    node.start();

    let miner_chain = Arc::clone(&chain);
    let miner = Miner::new(
        miner_chain,
        &tx_pool,
        config.signer.miner_private_key,
        bigint::H256::from(&config.signer.signer_private_key[..]),
        &network,
        &lock,
    );

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
