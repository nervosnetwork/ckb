use crate::{RpcServer, ServiceBuilder};
use ckb_app_config::{IndexerConfig, NetworkAlertConfig, NetworkConfig, RpcConfig, RpcModule};
use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_dao::DaoCalculator;
use ckb_dao_utils::genesis_dao_data;
use ckb_fee_estimator::FeeRate;
use ckb_indexer::{DefaultIndexerStore, IndexerStore};
use ckb_jsonrpc_types::{Block as JsonBlock, Uint64};
use ckb_network::{DefaultExitHandler, NetworkService, NetworkState};
use ckb_network_alert::alert_relayer::AlertRelayer;
use ckb_notify::NotifyService;
use ckb_shared::{
    shared::{Shared, SharedBuilder},
    Snapshot,
};
use ckb_store::ChainStore;
use ckb_sync::{SyncShared, Synchronizer};
use ckb_test_chain_utils::{always_success_cell, always_success_cellbase};
use ckb_types::{
    core::{
        capacity_bytes, cell::resolve_transaction, BlockBuilder, BlockView, Capacity,
        EpochNumberWithFraction, HeaderView, TransactionBuilder, TransactionView,
    },
    h256,
    packed::{AlertBuilder, CellDep, CellInput, CellOutputBuilder, OutPoint, RawAlertBuilder},
    prelude::*,
    H256,
};
use pretty_assertions::assert_eq as pretty_assert_eq;
use serde::{Deserialize, Serialize};
use serde_json::{from_reader, json, to_string, Map, Value};
use std::cell::RefCell;
use std::collections::HashSet;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

const GENESIS_TIMESTAMP: u64 = 1_557_310_743;
const GENESIS_TARGET: u32 = 0x2001_0000;
const EPOCH_REWARD: u64 = 125_000_000_000_000;
const CELLBASE_MATURITY: u64 = 0;
const ALERT_UNTIL_TIMESTAMP: u64 = 2_524_579_200;
const TARGET_HEIGHT: u64 = 1024;

thread_local! {
    // We store a cellbase for constructing a new transaction later
    static UNSPENT: RefCell<H256> = RefCell::new(h256!("0x0"));
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct JsonResponse {
    pub jsonrpc: String,
    pub id: usize,
    pub result: Option<Value>,
    pub error: Option<Value>,
}

// Construct `Consensus` with an always-success cell
//
// It is similar to `util::test-chain-utils::always_success_consensus`, but with hard-code
// genesis timestamp.
fn always_success_consensus() -> Consensus {
    let always_success_tx = always_success_transaction();
    let dao = genesis_dao_data(vec![&always_success_tx]).unwrap();
    let genesis = BlockBuilder::default()
        .timestamp(GENESIS_TIMESTAMP.pack())
        .compact_target(GENESIS_TARGET.pack())
        .dao(dao)
        .transaction(always_success_tx)
        .build();
    ConsensusBuilder::default()
        .genesis_block(genesis)
        .initial_primary_epoch_reward(Capacity::shannons(EPOCH_REWARD))
        .cellbase_maturity(EpochNumberWithFraction::from_full_value(CELLBASE_MATURITY))
        .build()
}

// Construct `Transaction` with an always-success cell
//
// The 1st transaction in genesis block, which contains an always_success_cell as the 1st output
fn always_success_transaction() -> TransactionView {
    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_cell_data.to_owned().pack())
        .witness(always_success_script.clone().into_witness())
        .build()
}

// Construct the next block based the given `parent`
fn next_block(shared: &Shared, parent: &HeaderView) -> BlockView {
    let snapshot: &Snapshot = &shared.snapshot();
    let epoch = {
        let last_epoch = snapshot
            .get_block_epoch(&parent.hash())
            .expect("current epoch exists");
        snapshot
            .next_epoch_ext(shared.consensus(), &last_epoch, parent)
            .unwrap_or(last_epoch)
    };
    let (_, reward) = snapshot.finalize_block_reward(parent).unwrap();
    let cellbase = always_success_cellbase(parent.number() + 1, reward.total, shared.consensus());

    // We store a cellbase for constructing a new transaction later
    if parent.number() > shared.consensus().finalization_delay_length() {
        UNSPENT.with(|unspent| {
            *unspent.borrow_mut() = cellbase.hash().unpack();
        });
    }

    let dao = {
        let resolved_cellbase =
            resolve_transaction(cellbase.clone(), &mut HashSet::new(), snapshot, snapshot).unwrap();
        DaoCalculator::new(shared.consensus(), shared.store())
            .dao_field(&[resolved_cellbase], parent)
            .unwrap()
    };
    BlockBuilder::default()
        .transaction(cellbase)
        .parent_hash(parent.hash())
        .number((parent.number() + 1).pack())
        .epoch(epoch.number_with_fraction(parent.number() + 1).pack())
        .timestamp((parent.timestamp() + 1).pack())
        .compact_target(epoch.compact_target().pack())
        .dao(dao)
        .build()
}

// Setup the running environment
fn setup_node(height: u64) -> (Shared, ChainController, RpcServer) {
    let (shared, table) = SharedBuilder::default()
        .consensus(always_success_consensus())
        .build()
        .unwrap();
    let chain_controller = ChainService::new(shared.clone(), table).start::<&str>(None);

    // Build chain, insert [1, height) blocks
    let mut parent = always_success_consensus().genesis_block;

    for _ in 0..height {
        let block = next_block(&shared, &parent.header());
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("processing new block should be ok");
        parent = block;
    }

    // Start network services
    let dir = tempfile::tempdir()
        .expect("create tempdir failed")
        .path()
        .to_path_buf();
    let network_controller = {
        let mut network_config = NetworkConfig::default();
        network_config.path = dir.clone();
        network_config.ping_interval_secs = 1;
        network_config.ping_timeout_secs = 1;
        network_config.connect_outbound_interval_secs = 1;
        let network_state =
            Arc::new(NetworkState::from_config(network_config).expect("Init network state failed"));
        NetworkService::new(
            Arc::clone(&network_state),
            Vec::new(),
            Vec::new(),
            shared.consensus().identify_name(),
            "0.1.0".to_string(),
            DefaultExitHandler::default(),
        )
        .start(Some("rpc-test-network"))
        .expect("Start network service failed")
    };
    let sync_shared = Arc::new(SyncShared::new(shared.clone(), Default::default()));
    let synchronizer = Synchronizer::new(chain_controller.clone(), Arc::clone(&sync_shared));
    let indexer_config = {
        let mut indexer_config = IndexerConfig::default();
        indexer_config.db.path = dir.join("indexer");
        let indexer_store = DefaultIndexerStore::new(&indexer_config, shared.clone());
        let (_, _, always_success_script) = always_success_cell();
        indexer_store.insert_lock_hash(&always_success_script.calc_script_hash(), Some(0));
        // use hardcoded TXN_ATTACH_BLOCK_NUMS (100) value here to setup testing data.
        (0..=height / 100).for_each(|_| indexer_store.sync_index_states());
        indexer_config
    };

    let notify_controller = NotifyService::new(Default::default()).start(Some("test"));
    let alert_notifier = {
        let alert_relayer = AlertRelayer::new(
            "0.1.0".to_string(),
            notify_controller,
            NetworkAlertConfig::default(),
        );
        let alert_notifier = alert_relayer.notifier();
        let alert = AlertBuilder::default()
            .raw(
                RawAlertBuilder::default()
                    .id(42u32.pack())
                    .min_version(Some("0.0.1".to_string()).pack())
                    .max_version(Some("1.0.0".to_string()).pack())
                    .priority(1u32.pack())
                    .notice_until((ALERT_UNTIL_TIMESTAMP * 1000).pack())
                    .message("An example alert message!".pack())
                    .build(),
            )
            .build();
        alert_notifier.lock().add(&alert);
        Arc::clone(alert_notifier)
    };

    // Start rpc services
    let rpc_config = RpcConfig {
        listen_address: "127.0.0.01:0".to_owned(),
        tcp_listen_address: None,
        ws_listen_address: None,
        max_request_body_size: 20_000_000,
        threads: None,
        // enable all rpc modules in unit test
        modules: vec![
            RpcModule::Net,
            RpcModule::Chain,
            RpcModule::Miner,
            RpcModule::Pool,
            RpcModule::Experiment,
            RpcModule::Stats,
            RpcModule::Indexer,
            RpcModule::IntegrationTest,
            RpcModule::Alert,
            RpcModule::Subscription,
            RpcModule::Debug,
        ],
        reject_ill_transactions: true,
        // enable deprecated rpc in unit test
        enable_deprecated_rpc: true,
    };

    let builder = ServiceBuilder::new(&rpc_config)
        .enable_chain(shared.clone())
        .enable_pool(
            shared.clone(),
            Arc::clone(&sync_shared),
            FeeRate::zero(),
            true,
        )
        .enable_miner(
            shared.clone(),
            network_controller.clone(),
            chain_controller.clone(),
            true,
        )
        .enable_net(network_controller.clone(), sync_shared)
        .enable_stats(shared.clone(), synchronizer, Arc::clone(&alert_notifier))
        .enable_experiment(shared.clone())
        .enable_integration_test(shared.clone(), network_controller, chain_controller.clone())
        .enable_indexer(&indexer_config, shared.clone())
        .enable_debug();
    let io_handler = builder.build();

    let rpc_server = RpcServer::new(rpc_config, io_handler, shared.notify_controller());

    (shared, chain_controller, rpc_server)
}

fn load_cases_from_file() -> Vec<Value> {
    let mut file_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    file_path.push("json");
    file_path.push("rpc.json");
    let file = File::open(file_path).expect("opening test data json");
    let content: Value = from_reader(file).expect("reading test data json");
    content.as_array().expect("load in array format").clone()
}

// Construct a transaction which use tip-cellbase as input cell
fn construct_transaction() -> TransactionView {
    let previous_output = OutPoint::new(UNSPENT.with(|unspent| unspent.borrow().clone()).pack(), 0);
    let input = CellInput::new(previous_output, 0);
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(always_success_cell().2.clone())
        .build();
    let cell_dep = CellDep::new_builder()
        .out_point(OutPoint::new(always_success_transaction().hash(), 0))
        .build();
    TransactionBuilder::default()
        .input(input)
        .output(output)
        .output_data(Default::default())
        .cell_dep(cell_dep)
        .header_dep(always_success_consensus().genesis_hash())
        .build()
}

// Construct the request of the given case
fn request_of(method: &str, params: Value) -> Value {
    let mut request = Map::new();
    request.insert("id".to_owned(), json!(1));
    request.insert("jsonrpc".to_owned(), json!("2.0"));
    request.insert("method".to_owned(), json!(method));
    request.insert("params".to_owned(), params);
    json!(request)
}

// Get the actual result of the given case
fn result_of(client: &reqwest::Client, uri: &str, method: &str, params: Value) -> Value {
    let request = request_of(method, params.clone());
    match client
        .post(uri)
        .json(&request)
        .send()
        .unwrap_or_else(|_| {
            panic!(
                "send request error, method: {:?}, params: {:?}",
                method, params
            )
        })
        .json::<JsonResponse>()
    {
        Err(err) => panic!("{} response error: {:?}", method, err),
        Ok(json) => match json.error {
            Some(error) => panic!("{} response error: {}", method, to_string(&error).unwrap()),
            None => json!(json.result),
        },
    }
}

// Get the expected params of the given case
fn params_of(shared: &Shared, method: &str) -> Value {
    let tip = {
        let snapshot = shared.snapshot();
        let tip_header = snapshot.tip_header();
        snapshot.get_block(&tip_header.hash()).unwrap()
    };
    let tip_number: Uint64 = tip.number().into();
    let tip_hash = json!(format!("{:#x}", Unpack::<H256>::unpack(&tip.hash())));
    let target_hash = {
        let snapshot = shared.snapshot();
        let target_number = tip.number() - snapshot.consensus().finalization_delay_length();
        let target_hash = snapshot.get_block_hash(target_number).unwrap();
        json!(format!("{:#x}", target_hash))
    };
    let (_, _, always_success_script) = always_success_cell();
    let always_success_script_hash = {
        let always_success_script_hash: H256 = always_success_script.calc_script_hash().unpack();
        json!(format!("{:#x}", always_success_script_hash))
    };
    let always_success_out_point = {
        let out_point = OutPoint::new(always_success_transaction().hash(), 0);
        let json_out_point: ckb_jsonrpc_types::OutPoint = out_point.into();
        json!(json_out_point)
    };
    let (transaction, transaction_hash) = {
        let transaction = construct_transaction();
        let transaction_hash: H256 = transaction.hash().unpack();
        let json_transaction: ckb_jsonrpc_types::Transaction = transaction.data().into();
        (
            json!(json_transaction),
            json!(format!("{:#x}", transaction_hash)),
        )
    };
    let params = match method {
        "get_tip_block_number"
        | "get_tip_header"
        | "get_current_epoch"
        | "get_blockchain_info"
        | "tx_pool_info"
        | "get_lock_hash_index_states"
        | "clear_tx_pool" => vec![],
        "get_epoch_by_number" => vec![json!("0x0")],
        "get_block_hash" | "get_block_by_number" | "get_header_by_number" => {
            vec![json!(tip_number)]
        }
        "get_block" | "get_header" | "get_cellbase_output_capacity_details" => vec![tip_hash],
        "get_block_economic_state" => vec![target_hash],
        "get_cells_by_lock_hash"
        | "get_live_cells_by_lock_hash"
        | "get_transactions_by_lock_hash" => {
            vec![always_success_script_hash, json!("0xa"), json!("0xe")]
        }
        "get_live_cell" => vec![always_success_out_point, json!(true)],
        "set_ban" => vec![
            json!("192.168.0.2"),
            json!("insert"),
            json!("0x1ac89236180"),
            json!(true),
            json!("set_ban example"),
        ],
        "add_node" => vec![
            json!("QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS"),
            json!("/ip4/192.168.2.100/tcp/8114"),
        ],
        "remove_node" => vec![json!("QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS")],
        "send_transaction" => vec![transaction, json!("passthrough")],
        "dry_run_transaction" | "_compute_transaction_hash" => vec![transaction],
        "get_transaction" => vec![transaction_hash],
        "index_lock_hash" => vec![json!(always_success_script_hash), json!("0x400")],
        "deindex_lock_hash" | "get_capacity_by_lock_hash" => {
            vec![json!(always_success_script_hash)]
        }
        "_compute_code_hash" => vec![json!("0x123456")],
        "_compute_script_hash" => {
            let script = always_success_script.clone();
            let json_script: ckb_jsonrpc_types::Script = script.into();
            vec![json!(json_script)]
        }
        "estimate_fee_rate" => vec![json!("0xa")],
        "submit_block" => {
            let json_block: JsonBlock = tip.data().into();
            vec![json!("example"), json!(json_block)]
        }
        method => {
            panic!("Unknown method: {}", method);
        }
    };
    json!(params)
}

// Print the expected documentation based the actual results
fn print_document(params: Option<&Vec<(String, Value)>>, result: Option<&Vec<(String, Value)>>) {
    let is_params = params.is_some();
    let document: Vec<_> = load_cases_from_file()
        .iter_mut()
        .enumerate()
        .map(|(i, case)| {
            let object = case.as_object_mut().unwrap();
            if is_params {
                object.insert(
                    "params".to_string(),
                    params.unwrap().get(i).unwrap().clone().1,
                );
            } else {
                object.insert(
                    "result".to_string(),
                    result.unwrap().get(i).unwrap().clone().1,
                );
            }
            json!(object)
        })
        .collect();
    println!("\n\n###################################");
    println!("Expected RPC Document is written into rpc/json/rpc.expect.json");
    println!("Check full diff using following commands:");
    println!("    diff rpc/json/rpc.json rpc/json/rpc.expect.json");
    println!("###################################\n\n");

    let mut out_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    out_path.push("json");
    out_path.push("rpc.expect.json");

    let buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut ser = serde_json::Serializer::with_formatter(buf, formatter);
    document.serialize(&mut ser).unwrap();

    std::fs::write(out_path, String::from_utf8(ser.into_inner()).unwrap())
        .expect("Write to rpc/json/rpc.expect.json");
}

#[test]
fn test_rpc() {
    let (shared, _chain_controller, server) = setup_node(TARGET_HEIGHT);
    let client = reqwest::Client::new();
    let uri = format!(
        "http://{}:{}/",
        server.http_address().ip(),
        server.http_address().port()
    );

    // Assert the params of jsonrpc requests
    {
        let mut expected = Vec::new();
        let mut actual = Vec::new();
        load_cases_from_file().iter().for_each(|case| {
            let method = case
                .get("method")
                .expect("get method")
                .as_str()
                .unwrap()
                .to_string();
            let params = case.get("params").expect("get params");
            actual.push((method.clone(), params.clone()));
            if case.get("skip").unwrap_or(&json!(false)).as_bool().unwrap() {
                expected.push((method, params.clone()));
            } else {
                expected.push((method.clone(), params_of(&shared, &method)));
            }
        });
        if actual != expected {
            print_document(Some(&expected), None);
            pretty_assert_eq!(actual, expected, "Assert params of jsonrpc",);
        }
    }

    // Assert the results of jsonrpc responses
    {
        let mut expected = Vec::new();
        let mut actual = Vec::new();
        load_cases_from_file().iter().for_each(|case| {
            let method = case
                .get("method")
                .expect("get method")
                .as_str()
                .unwrap()
                .to_string();
            let params = case.get("params").expect("get params").clone();
            let result = case.get("result").expect("get result").clone();
            if case.get("skip").unwrap_or(&json!(false)).as_bool().unwrap() {
                expected.push((method.clone(), result.clone()));
            } else {
                expected.push((method.clone(), result_of(&client, &uri, &method, params)));
            }
            actual.push((method, result));
        });
        if actual != expected {
            print_document(None, Some(&expected));
            pretty_assert_eq!(actual, expected, "Assert results of jsonrpc",);
        }
    }
}
