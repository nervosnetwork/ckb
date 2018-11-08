use super::super::helper::wait_for_exit;
use super::super::Setup;
use bigint::H256;
use chain::cachedb::CacheDB;
use chain::chain::{ChainBuilder, ChainProvider};
use chain::store::ChainKVStore;
use ckb_notify::Notify;
use ckb_pow::PowEngine;
use clap::ArgMatches;
use core::script::Script;
use core::transaction::{CellInput, OutPoint, Transaction, TransactionBuilder};
use crypto::secp::{Generator, Privkey};
use db::diskdb::RocksDB;
use faster_hex::{hex_string, hex_to};
use hash::sha3_256;
use logger;
use miner::Miner;
use network::CKBProtocol;
use network::NetworkConfig;
use network::NetworkService;
use pool::TransactionPool;
use rpc::RpcServer;
use serde_json;
use std::io::Write;
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

    let pow_engine = setup.chain_spec.pow_engine();
    let synchronizer = Arc::new(Synchronizer::new(&chain, &pow_engine, setup.configs.sync));

    let tx_pool = TransactionPool::new(setup.configs.pool, Arc::clone(&chain), notify.clone());
    let relayer = Arc::new(Relayer::new(&chain, &pow_engine, &tx_pool));

    let network_config = NetworkConfig::from(setup.configs.network);
    let protocol_base_name = "ckb";
    let protocols = vec![
        CKBProtocol::new(
            protocol_base_name.to_string(),
            synchronizer as Arc<_>,
            SYNC_PROTOCOL_ID,
            &[1][..],
        ),
        CKBProtocol::new(
            protocol_base_name.to_string(),
            relayer as Arc<_>,
            RELAY_PROTOCOL_ID,
            &[1][..],
        ),
    ];
    let network = Arc::new(
        NetworkService::run_in_thread(&network_config, protocols)
            .expect("Create and start network"),
    );

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

    let rpc_server = RpcServer {
        config: setup.configs.rpc,
    };

    setup_rpc(rpc_server, &pow_engine, &network, &chain, &tx_pool);

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");

    logger::flush();
}

#[cfg(feature = "integration_test")]
fn setup_rpc<C: ChainProvider + 'static>(
    server: RpcServer,
    pow: &Arc<dyn PowEngine>,
    network: &Arc<NetworkService>,
    chain: &Arc<C>,
    tx_pool: &Arc<TransactionPool<C>>,
) {
    use ckb_pow::Clicker;

    let network = Arc::clone(network);
    let chain = Arc::clone(chain);
    let tx_pool = Arc::clone(tx_pool);

    let pow = pow.as_ref().as_any();

    let pow = match pow.downcast_ref::<Clicker>() {
        Some(pow) => Arc::new(pow.clone()),
        None => panic!("pow isn't a Clicker!"),
    };

    let _ = thread::Builder::new().name("rpc".to_string()).spawn({
        move || {
            server.start(network, chain, tx_pool, pow);
        }
    });
}

#[cfg(not(feature = "integration_test"))]
#[cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]
fn setup_rpc<C: ChainProvider + 'static>(
    server: RpcServer,
    _pow: &Arc<dyn PowEngine>,
    network: &Arc<NetworkService>,
    chain: &Arc<C>,
    tx_pool: &Arc<TransactionPool<C>>,
) {
    let network = Arc::clone(network);
    let chain = Arc::clone(chain);
    let tx_pool = Arc::clone(tx_pool);
    let _ = thread::Builder::new().name("rpc".to_string()).spawn({
        move || {
            server.start(network, chain, tx_pool);
        }
    });
}

pub fn sign(setup: &Setup, matches: &ArgMatches) {
    let consensus = setup.chain_spec.to_consensus().unwrap();
    let system_cell_tx = &consensus.genesis_block().commit_transactions()[0];
    let system_cell_data_hash = system_cell_tx.outputs()[0].data_hash();
    let system_cell_tx_hash = system_cell_tx.hash();
    let system_cell_outpoint = OutPoint::new(system_cell_tx_hash, 0);

    let privkey: Privkey = value_t!(matches.value_of("private-key"), H256)
        .unwrap_or_else(|e| e.exit())
        .into();
    let pubkey = privkey.pubkey().unwrap();
    let json =
        value_t!(matches.value_of("unsigned-transaction"), String).unwrap_or_else(|e| e.exit());
    let transaction: Transaction = serde_json::from_str(&json).unwrap();
    let mut inputs = Vec::new();
    for unsigned_input in transaction.inputs() {
        let mut bytes = vec![];
        for argument in &unsigned_input.unlock.args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex signature");

        let mut new_args = vec![hex_signature];
        new_args.extend_from_slice(&unsigned_input.unlock.args);

        let pubkey_ser = pubkey.serialize();
        let mut hex_pubkey = vec![0; pubkey_ser.len() * 2];
        hex_to(&pubkey_ser, &mut hex_pubkey).expect("hex pubkey");
        let script = Script::new(
            0,
            new_args,
            Some(system_cell_data_hash),
            None,
            vec![hex_pubkey],
        );
        let signed_input = CellInput::new(unsigned_input.previous_output, script);
        inputs.push(signed_input);
    }
    // First, add verify system cell as a dep
    // Then, sign each input
    let result = TransactionBuilder::default()
        .transaction(transaction)
        .dep(system_cell_outpoint)
        .inputs_clear()
        .inputs(inputs)
        .build();

    println!("{}", serde_json::to_string(&result).unwrap());
}

pub fn type_hash(setup: &Setup, matches: &ArgMatches) {
    let consensus = setup.chain_spec.to_consensus().unwrap();
    let system_cell_tx = &consensus.genesis_block().commit_transactions()[0];
    let system_cell_data_hash = system_cell_tx.outputs()[0].data_hash();

    let privkey: Privkey = value_t!(matches.value_of("private-key"), H256)
        .unwrap_or_else(|e| e.exit())
        .into();
    let pubkey = privkey.pubkey().unwrap();

    let pubkey_ser = pubkey.serialize();
    let mut hex_pubkey = vec![0; pubkey_ser.len() * 2];
    hex_to(&pubkey_ser, &mut hex_pubkey).expect("hex pubkey");

    let script = Script::new(
        0,
        Vec::new(),
        Some(system_cell_data_hash),
        None,
        vec![hex_pubkey],
    );
    println!("{}", hex_string(&script.type_hash()).expect("hex string"));
}

pub fn keygen() {
    let result: H256 = Generator::new().random_privkey().into();
    println!("{:?}", result)
}
