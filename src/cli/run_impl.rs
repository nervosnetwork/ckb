use adapter::{ChainToNetAndPoolAdapter, NetToChainAndPoolAdapter, PoolToChainAdapter,
              PoolToNetAdapter};
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
use pool::*;
use rpc::RpcServer;
use std::sync::Arc;
use std::thread;
use util::{Condvar, Mutex};

pub fn run(config: Config) {
    logger::init(config.logger_config()).expect("Init Logger");

    info!(target: "main", "Value for config: {:?}", config);

    let lock = Arc::new(Mutex::new(()));

    let db = CacheKeyValueDB::new(RocksKeyValueDB::open(&config.dirs.db));
    let store = ChainKVStore { db: Box::new(db) };

    let pool_net_adapter = Arc::new(PoolToNetAdapter::new());

    let pool_chain_adapter = Arc::new(PoolToChainAdapter::new());

    let tx_pool = Arc::new(TransactionPool::new(
        PoolConfig::default(),
        Arc::<PoolToChainAdapter>::clone(&pool_chain_adapter),
        Arc::<PoolToNetAdapter>::clone(&pool_net_adapter),
    ));

    let chain_adapter = Arc::new(ChainToNetAndPoolAdapter::new(Arc::clone(&tx_pool)));

    let chain = Arc::new(
        Chain::init(
            store,
            Arc::clone(&chain_adapter),
            &chain::genesis::genesis_dev(),
        ).unwrap(),
    );

    let kg = Arc::new(config.key_group());

    let net_adapter =
        NetToChainAndPoolAdapter::new(kg, &chain, Arc::clone(&tx_pool), Arc::clone(&lock));

    let network = Arc::new(Network::new(net_adapter, config.network));

    chain_adapter.init(&network);
    pool_net_adapter.init(&network);
    pool_chain_adapter.init(&chain);

    let miner = Miner {
        tx_pool,
        lock,
        chain: Arc::clone(&chain),
        miner_key: config.signer.miner_private_key,
        signer_key: bigint::H256::from(&config.signer.signer_private_key[..]),
    };

    let network_clone = Arc::clone(&network);
    let _ = thread::Builder::new()
        .name("network".to_string())
        .spawn(move || {
            network_clone.start();
        });

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
