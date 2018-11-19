use super::super::Spec;
use chain::cachedb::CacheDB;
use chain::chain::ChainProvider;
use chain::chain::{Chain, ChainBuilder};
use chain::store::ChainKVStore;
use ctrlc;
use db::diskdb::RocksDB;
use ethash::Ethash;
use logger;
use miner::miner::Miner;
use nervos_notify::Notify;
use nervos_verification::EthashVerifier;
use network::NetworkService;
use pool::*;
use rpc::RpcServer;
use std::sync::Arc;
use std::thread;
use sync::protocol::{RelayProtocol, SyncProtocol};
use sync::synchronizer::Synchronizer;
use sync::{RELAY_PROTOCOL_ID, SYNC_PROTOCOL_ID};
use util::{Condvar, Mutex};

pub fn run(spec: Spec) {
    logger::init(spec.configs.logger.clone()).expect("Init Logger");

    info!(target: "main", "Value for spec: {:?}", spec);
    let rocks_db_path = spec.dirs.join("db");
    let ethash = spec
        .configs
        .miner
        .clone()
        .ethash_path
        .map(|path| Arc::new(Ethash::new(path)));

    let notify = Notify::new();

    let chain = {
        let mut builder = ChainBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_rocks(&rocks_db_path)
            .config(spec.configs.chain)
            .notify(notify.clone());
        Arc::new(builder.build().unwrap())
    };

    info!(target: "main", "chain genesis hash: {:?}", chain.genesis_hash());

    let synchronizer = Synchronizer::new(
        &chain,
        notify.clone(),
        ethash.clone().map(|e| EthashVerifier::new(&e)),
        spec.configs.sync,
    );

    let chain1 = Arc::<Chain<ChainKVStore<CacheDB<RocksDB>>>>::clone(&chain);
    let tx_pool = TransactionPool::new(PoolConfig::default(), chain1, notify.clone());

    let network =
        Arc::new(NetworkService::new(spec.configs.network, Option::None).expect("Create network"));

    let sync_protocol = Arc::new(SyncProtocol::new(synchronizer.clone()));
    let sync_protocol_clone = Arc::clone(&sync_protocol);

    let _ = thread::Builder::new()
        .name("sync".to_string())
        .spawn(move || {
            sync_protocol_clone.start();
        });

    let relay_protocol = Arc::new(RelayProtocol::new(synchronizer, &tx_pool));
    network.register_protocol(sync_protocol, SYNC_PROTOCOL_ID, &[(1, 0)]);
    network.register_protocol(relay_protocol, RELAY_PROTOCOL_ID, &[(1, 0)]);
    network.start().expect("Start network service");

    let miner_chain = Arc::clone(&chain);
    let miner = Miner::new(
        spec.configs.miner,
        miner_chain,
        &tx_pool,
        &network,
        ethash,
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
    let tx_pool_clone = Arc::clone(&tx_pool);
    let _ = thread::Builder::new()
        .name("rpc".to_string())
        .spawn(move || {
            rpc_server.start(network_clone, chain_clone, tx_pool_clone);
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
