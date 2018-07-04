use super::super::Spec;
use chain::cachedb::CacheDB;
use chain::chain::Chain;
use chain::store::ChainKVStore;
use chain::COLUMNS;
use chain::COLUMN_BLOCK_HEADER;
use ctrlc;
use db::diskdb::RocksDB;
use ethash::Ethash;
use logger;
use miner::miner::Miner;
use nervos_notify::Notify;
use network::NetworkService;
use pool::*;
use rpc::RpcServer;
use std::sync::Arc;
use std::thread;
use sync::chain::Chain as SyncChain;
use sync::protocol::{RelayProtocol, SyncProtocol, RELAY_PROTOCOL_ID, SYNC_PROTOCOL_ID};
use util::{Condvar, Mutex};

pub fn run(spec: Spec) {
    logger::init(spec.configs.logger.clone()).expect("Init Logger");

    info!(target: "main", "Value for spec: {:?}", spec);

    let db = CacheDB::new(
        RocksDB::open(&spec.dirs.join("db"), COLUMNS),
        &[(COLUMN_BLOCK_HEADER.unwrap(), 4096)],
    );
    let store = ChainKVStore { db };

    let ethash = Arc::new(Ethash::new(spec.dirs.join("ethash")));
    let notify = Notify::new();
    let chain = Arc::new(Chain::init(store, spec.configs.chain, &ethash).unwrap());
    let sync_chain = Arc::new(SyncChain::new(&chain, notify.clone()));

    let tx_pool = Arc::new(TransactionPool::new(
        PoolConfig::default(),
        &chain,
        notify.clone(),
    ));

    let network =
        Arc::new(NetworkService::new(spec.configs.network, Option::None).expect("Create network"));
    let sync_protocol = Arc::new(SyncProtocol::new(&sync_chain));
    let relay_protocol = Arc::new(RelayProtocol::new(&sync_chain, &tx_pool));
    network.register_protocol(sync_protocol, SYNC_PROTOCOL_ID, &[(1, 0)]);
    network.register_protocol(relay_protocol, RELAY_PROTOCOL_ID, &[(1, 0)]);
    network.start().expect("Start network service");

    let miner_chain = Arc::clone(&chain);
    let miner = Miner::new(
        spec.configs.miner,
        miner_chain,
        &tx_pool,
        &network,
        &ethash,
        &notify,
    );

    let _ = thread::Builder::new()
        .name("miner".to_string())
        .spawn(move || {
            miner.run_loop();
        });

    let rpc_server = RpcServer {
        config: spec.configs.rpc,
    };
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
