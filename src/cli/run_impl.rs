use super::super::helper::wait_for_exit;
use super::super::Setup;
use bigint::H256;
use chain::cachedb::CacheDB;
use chain::chain::{ChainBuilder, ChainProvider};
use chain::store::ChainKVStore;
#[cfg(feature = "integration_test")]
use chain::Clicker;
#[cfg(feature = "pow_engine_cuckoo")]
use chain::CuckooEngine;
#[cfg(feature = "pow_engine_dummy")]
use chain::DummyPowEngine;
#[cfg(feature = "pow_engine_ethash")]
use chain::EthashEngine;
use ckb_notify::Notify;
use clap::ArgMatches;
use core::script::Script;
use core::transaction::{CellInput, OutPoint, Transaction};
use crypto::secp::{Generator, Privkey};
use db::diskdb::RocksDB;
use hash::sha3_256;
use logger;
use miner::Miner;
use network::NetworkConfiguration;
use network::NetworkService;
use pool::TransactionPool;
use rpc::{Config as RpcConfig, RpcServer};
use rustc_hex::ToHex;
use serde_json;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use sync::{Relayer, Synchronizer, RELAY_PROTOCOL_ID, SYNC_PROTOCOL_ID};

pub fn run(setup: Setup) {
    logger::init(setup.configs.logger.clone()).expect("Init Logger");
    info!(target: "main", "Value for setup: {:?}", setup);

    let consensus = setup.chain_spec.to_consensus().unwrap();
    let db_path = setup.dirs.join("db");
    let notify = Notify::new();

    let chain = {
        let mut builder = ChainBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_rocks(&db_path)
            .consensus(consensus.clone())
            .notify(notify.clone());
        Arc::new(builder.build().unwrap())
    };

    info!(target: "main", "chain genesis hash: {:?}", chain.genesis_hash());

    let pow_engine = build_pow_engine(setup.dirs.join("pow"));
    let synchronizer = Arc::new(Synchronizer::new(&chain, &pow_engine, setup.configs.sync));

    let tx_pool = TransactionPool::new(setup.configs.pool, Arc::clone(&chain), notify.clone());
    let relayer = Arc::new(Relayer::new(&chain, &pow_engine, &tx_pool));

    let network_config = NetworkConfiguration::from(setup.configs.network);
    let protocols = vec![
        (synchronizer as Arc<_>, SYNC_PROTOCOL_ID, &[(1, 1)][..]),
        (relayer as Arc<_>, RELAY_PROTOCOL_ID, &[(1, 1)][..]),
    ];
    let network =
        Arc::new(NetworkService::new(network_config, protocols).expect("Create and start network"));

    let _ = thread::Builder::new().name("miner".to_string()).spawn({
        let miner_clone = Arc::clone(&chain);

        let mut miner = Miner::new(
            setup.configs.miner,
            miner_clone,
            &pow_engine,
            &tx_pool,
            &network,
            &notify,
        );

        move || {
            miner.start();
        }
    });

    let rpc_server = build_rpc(
        setup.configs.rpc,
        if cfg!(feature = "integration_test") {
            Some(pow_engine)
        } else {
            None
        },
    );

    let _ = thread::Builder::new().name("rpc".to_string()).spawn({
        let network_clone = Arc::clone(&network);
        let chain_clone = Arc::clone(&chain);
        let tx_pool_clone = Arc::clone(&tx_pool);
        move || {
            rpc_server.start(network_clone, chain_clone, tx_pool_clone);
        }
    });

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");

    logger::flush();
}

#[cfg(feature = "pow_engine_cuckoo")]
fn build_pow_engine<P: AsRef<Path>>(_pow_data_path: P) -> Arc<CuckooEngine> {
    Arc::new(CuckooEngine::new())
}

#[cfg(feature = "pow_engine_ethash")]
fn build_pow_engine<P: AsRef<Path>>(pow_data_path: P) -> Arc<EthashEngine> {
    Arc::new(EthashEngine::new(pow_data_path))
}

#[cfg(feature = "pow_engine_dummy")]
fn build_pow_engine<P: AsRef<Path>>(_pow_data_path: P) -> Arc<DummyPowEngine> {
    Arc::new(DummyPowEngine::new())
}

#[cfg(feature = "integration_test")]
fn build_pow_engine<P: AsRef<Path>>(_pow_data_path: P) -> Arc<Clicker> {
    Arc::new(Clicker::new())
}

#[cfg(feature = "integration_test")]
fn build_rpc(config: RpcConfig, mut pow: Option<Arc<Clicker>>) -> RpcServer {
    RpcServer {
        config,
        pow: pow.take().expect("must assign clicker engine"),
    }
}

#[cfg(feature = "pow_engine_cuckoo")]
#[cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]
fn build_rpc(config: RpcConfig, _pow: Option<Arc<CuckooEngine>>) -> RpcServer {
    RpcServer { config }
}

#[cfg(feature = "pow_engine_ethash")]
#[cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]
fn build_rpc(config: RpcConfig, _pow: Option<Arc<EthashEngine>>) -> RpcServer {
    RpcServer { config }
}

#[cfg(feature = "pow_engine_dummy")]
#[cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]
fn build_rpc(config: RpcConfig, _pow: Option<Arc<DummyPowEngine>>) -> RpcServer {
    RpcServer { config }
}

pub fn sign(setup: &Setup, matches: &ArgMatches) {
    let consensus = setup.chain_spec.to_consensus().unwrap();
    let system_cell_tx_hash = consensus.genesis_block().commit_transactions[0].hash();
    let system_cell_outpoint = OutPoint::new(system_cell_tx_hash, 0);

    let privkey: Privkey = value_t!(matches.value_of("private-key"), H256)
        .unwrap_or_else(|e| e.exit())
        .into();
    let pubkey = privkey.pubkey().unwrap();
    let json =
        value_t!(matches.value_of("unsigned-transaction"), String).unwrap_or_else(|e| e.exit());
    let transaction: Transaction = serde_json::from_str(&json).unwrap();
    let mut result = transaction.clone();

    // First, add verify system cell as a dep
    result.deps.push(system_cell_outpoint);
    // Then, sign each input
    let mut inputs = Vec::new();
    for unsigned_input in result.inputs {
        let mut bytes = vec![];
        for argument in &unsigned_input.unlock.arguments {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let mut new_arguments = vec![signature.serialize_der().to_hex().into_bytes()];
        new_arguments.extend_from_slice(&unsigned_input.unlock.arguments);
        let script = Script::new(
            0,
            new_arguments,
            Some(system_cell_outpoint),
            None,
            vec![pubkey.serialize().to_hex().into_bytes()],
        );
        let signed_input = CellInput::new(unsigned_input.previous_output, script);
        inputs.push(signed_input);
    }
    result.inputs = inputs;
    println!("{}", serde_json::to_string(&result).unwrap());
}

pub fn redeem_script_hash(setup: &Setup, matches: &ArgMatches) {
    let consensus = setup.chain_spec.to_consensus().unwrap();
    let system_cell_tx_hash = consensus.genesis_block().commit_transactions[0].hash();
    let system_cell_outpoint = OutPoint::new(system_cell_tx_hash, 0);

    let privkey: Privkey = value_t!(matches.value_of("private-key"), H256)
        .unwrap_or_else(|e| e.exit())
        .into();
    let pubkey = privkey.pubkey().unwrap();

    let script = Script::new(
        0,
        Vec::new(),
        Some(system_cell_outpoint),
        None,
        vec![pubkey.serialize().to_hex().into_bytes()],
    );
    println!("{}", script.redeem_script_hash().to_hex());
}

pub fn keygen() {
    let result: H256 = Generator::new().random_privkey().into();
    println!("{:?}", result)
}
