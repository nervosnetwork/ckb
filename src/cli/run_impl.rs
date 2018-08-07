use super::super::helper::wait_for_exit;
use super::super::Setup;
use bigint::H256;
use chain::cachedb::CacheDB;
use chain::chain::ChainProvider;
use chain::chain::{Chain, ChainBuilder};
use chain::store::ChainKVStore;
use ckb_notify::Notify;
use ckb_verification::EthashVerifier;
use clap::ArgMatches;
use core::transaction::Transaction;
use crypto::secp::{Generator, Privkey};
use db::diskdb::RocksDB;
use ethash::Ethash;
use logger;
use miner::miner::Miner;
use network::NetworkConfiguration;
use network::NetworkService;
use pool::{PoolConfig, TransactionPool};
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::Client;
use rpc::RpcServer;
use script::TransactionInputSigner;
use serde_json::{self, Value};
use std::str::FromStr;
use std::sync::Arc;
use std::thread;
use sync::protocol::{RelayProtocol, SyncProtocol};
use sync::synchronizer::Synchronizer;
use sync::{RELAY_PROTOCOL_ID, SYNC_PROTOCOL_ID};

pub fn run(setup: Setup) {
    logger::init(setup.configs.logger.clone()).expect("Init Logger");

    info!(target: "main", "Value for setup: {:?}", setup);
    let rocks_db_path = setup.dirs.join("db");
    let ethash = setup
        .configs
        .miner
        .clone()
        .ethash_path
        .map(|path| Arc::new(Ethash::new(path)));

    let notify = Notify::new();

    let chain = {
        let mut builder = ChainBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_rocks(&rocks_db_path)
            .consensus(setup.chain_spec.to_consensus())
            .notify(notify.clone());
        Arc::new(builder.build().unwrap())
    };

    info!(target: "main", "chain genesis hash: {:?}", chain.genesis_hash());

    let synchronizer = Synchronizer::new(
        &chain,
        ethash.clone().map(|e| EthashVerifier::new(&e)),
        setup.configs.sync,
    );

    let chain1 = Arc::<Chain<ChainKVStore<CacheDB<RocksDB>>>>::clone(&chain);
    let tx_pool = TransactionPool::new(PoolConfig::default(), chain1, notify.clone());

    let network_config = NetworkConfiguration::from(setup.configs.network);
    let network = Arc::new(NetworkService::new(network_config, None).expect("Create network"));

    let sync_protocol = Arc::new(SyncProtocol::new(synchronizer.clone()));
    let sync_protocol_clone = Arc::clone(&sync_protocol);

    let _ = thread::Builder::new()
        .name("sync".to_string())
        .spawn(move || {
            sync_protocol_clone.start();
        });

    let relay_protocol = Arc::new(RelayProtocol::new(synchronizer, &tx_pool));
    let protocols = vec![
        (sync_protocol as Arc<_>, SYNC_PROTOCOL_ID, &[(1, 1)][..]),
        (relay_protocol as Arc<_>, RELAY_PROTOCOL_ID, &[(1, 1)][..]),
    ];
    network.start(protocols).expect("Start network service");

    let miner_chain = Arc::clone(&chain);
    let miner = Miner::new(
        setup.configs.miner,
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
        config: setup.configs.rpc,
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

pub fn rpc(matches: &ArgMatches) {
    let uri = matches.value_of("uri").unwrap_or("http://localhost:3030");
    let method = matches.value_of("method").unwrap_or("get_tip_header");
    let params = matches.value_of("params").unwrap_or("null");
    let body = format!(
        r#"{{"id": 1, "jsonrpc": "2.0", "method": "{}", "params": {}}}"#,
        method, params
    );

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let result: Value = Client::new()
        .post(uri)
        .headers(headers)
        .body(body)
        .send()
        .unwrap()
        .json()
        .unwrap();

    println!("{}", result)
}

pub fn sign(matches: &ArgMatches) {
    let privkey: Privkey = H256::from_str(matches.value_of("private-key").unwrap())
        .unwrap()
        .into();
    let json = matches.value_of("unsigned-transaction").unwrap();
    let transaction: Transaction = serde_json::from_str(json).unwrap();
    let mut result = transaction.clone();
    let mut inputs = Vec::new();
    let signer: TransactionInputSigner = transaction.into();
    for index in 0..result.inputs.len() {
        inputs.push(signer.signed_input(&privkey, index));
    }
    result.inputs = inputs;
    println!("{}", serde_json::to_string(&result).unwrap())
}

pub fn keygen() {
    let result: H256 = Generator::new().random_privkey().into();
    println!("{:?}", result)
}
