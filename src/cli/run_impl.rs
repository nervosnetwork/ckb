use super::super::helper::wait_for_exit;
use super::super::Setup;
use bigint::H256;
use chain::chain::{ChainBuilder, ChainController};
use ckb_notify::NotifyService;
use ckb_pow::PowEngine;
use clap::ArgMatches;
use core::script::Script;
use core::transaction::{CellInput, OutPoint, Transaction, TransactionBuilder};
use crypto::secp::{Generator, Privkey};
use db::diskdb::RocksDB;
use faster_hex::{hex_string, hex_to};
use hash::sha3_256;
use logger;
use miner::MinerService;
use network::CKBProtocol;
use network::NetworkConfig;
use network::NetworkService;
use pool::txs_pool::{TransactionPoolController, TransactionPoolService};
use rpc::{RpcController, RpcServer, RpcService};
use serde_json;
use shared::cachedb::CacheDB;
use shared::index::ChainIndex;
use shared::shared::{ChainProvider, Shared, SharedBuilder};
use shared::store::ChainKVStore;
use std::io::Write;
use std::sync::Arc;
use std::thread;
use sync::{Relayer, Synchronizer, RELAY_PROTOCOL_ID, SYNC_PROTOCOL_ID};
use verification::BlockVerifier;

pub fn run(setup: Setup) {
    logger::init(setup.configs.logger.clone()).expect("Init Logger");
    info!(target: "main", "Value for setup: {:?}", setup);

    let consensus = setup.chain_spec.to_consensus().unwrap();
    let pow_engine = setup.chain_spec.pow_engine();
    let db_path = setup.dirs.join("db");

    let shared = SharedBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_rocks(&db_path)
        .consensus(consensus)
        .build();

    let (_handle, notify) = NotifyService::default().start(Some("notify"));
    let (chain_controller, chain_receivers) = ChainController::new();
    let (tx_pool_controller, tx_pool_receivers) = TransactionPoolController::new();
    let (rpc_controller, rpc_receivers) = RpcController::new();

    let chain_service = ChainBuilder::new(shared.clone())
        .notify(notify.clone())
        .build();
    let _handle = chain_service.start(Some("ChainService"), chain_receivers);

    info!(target: "main", "chain genesis hash: {:?}", shared.genesis_hash());

    let block_verifier = BlockVerifier::new(
        shared.clone(),
        shared.consensus().clone(),
        Arc::clone(&pow_engine),
    );

    let tx_pool_service =
        TransactionPoolService::new(setup.configs.pool, shared.clone(), notify.clone());
    let _handle = tx_pool_service.start(Some("TransactionPoolService"), tx_pool_receivers);

    let rpc_service = RpcService::new(shared.clone(), tx_pool_controller.clone());
    let _handle = rpc_service.start(Some("RpcService"), rpc_receivers, &notify);

    let synchronizer = Arc::new(Synchronizer::new(
        chain_controller.clone(),
        shared.clone(),
        Arc::clone(&pow_engine),
        block_verifier.clone(),
        setup.configs.sync,
    ));

    let relayer = Arc::new(Relayer::new(
        chain_controller.clone(),
        shared.clone(),
        Arc::clone(&pow_engine),
        tx_pool_controller.clone(),
        block_verifier,
    ));

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

    let miner_service = MinerService::new(
        setup.configs.miner,
        Arc::clone(&pow_engine),
        &shared,
        chain_controller.clone(),
        rpc_controller.clone(),
        Arc::clone(&network),
        &notify,
    );
    let _handle = miner_service.start(Some("MinerService"));

    let rpc_server = RpcServer {
        config: setup.configs.rpc,
    };

    setup_rpc(
        rpc_server,
        rpc_controller,
        Arc::clone(&pow_engine),
        Arc::clone(&network),
        shared,
        tx_pool_controller,
    );

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");

    logger::flush();
}

#[cfg(feature = "integration_test")]
fn setup_rpc<CI: ChainIndex + 'static>(
    server: RpcServer,
    rpc: RpcController,
    pow: Arc<dyn PowEngine>,
    network: Arc<NetworkService>,
    shared: Shared<CI>,
    tx_pool: TransactionPoolController,
) {
    use ckb_pow::Clicker;

    let pow = pow.as_ref().as_any();

    let pow = match pow.downcast_ref::<Clicker>() {
        Some(pow) => Arc::new(pow.clone()),
        None => panic!("pow isn't a Clicker!"),
    };

    let _ = thread::Builder::new().name("rpc".to_string()).spawn({
        move || {
            server.start(network, shared, tx_pool, rpc, pow);
        }
    });
}

#[cfg(not(feature = "integration_test"))]
#[cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]
fn setup_rpc<CI: ChainIndex + 'static>(
    server: RpcServer,
    rpc: RpcController,
    _pow: Arc<dyn PowEngine>,
    network: Arc<NetworkService>,
    shared: Shared<CI>,
    tx_pool: TransactionPoolController,
) {
    let _ = thread::Builder::new().name("rpc".to_string()).spawn({
        move || {
            server.start(network, shared, tx_pool, rpc);
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
