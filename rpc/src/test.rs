use crate::module::{
    ChainRpc, ChainRpcImpl, ExperimentRpc, ExperimentRpcImpl, IndexerRpc, IndexerRpcImpl,
    NetworkRpc, NetworkRpcImpl, PoolRpc, PoolRpcImpl, StatsRpc, StatsRpcImpl,
};
use crate::RpcServer;
use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::cell::resolve_transaction;
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::transaction::{
    CellDep, CellInput, CellOutput, CellOutputBuilder, OutPoint, Transaction, TransactionBuilder,
};
use ckb_core::{alert::AlertBuilder, capacity_bytes, Bytes, Capacity};
use ckb_dao::DaoCalculator;
use ckb_dao_utils::genesis_dao_data;
use ckb_db::DBConfig;
use ckb_indexer::{DefaultIndexerStore, IndexerStore};
use ckb_network::{NetworkConfig, NetworkService, NetworkState};
use ckb_network_alert::{
    alert_relayer::AlertRelayer, config::SignatureConfig as AlertSignatureConfig,
};
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_sync::{SyncSharedState, Synchronizer};
use ckb_test_chain_utils::{always_success_cell, always_success_cellbase};
use ckb_traits::chain_provider::ChainProvider;
use jsonrpc_core::IoHandler;
use jsonrpc_http_server::ServerBuilder;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use pretty_assertions::assert_eq as pretty_assert_eq;
use reqwest;
use serde_derive::{Deserialize, Serialize};
use serde_json::{from_reader, json, to_string, to_string_pretty, Map, Value};
use std::cell::RefCell;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

const GENESIS_TIMESTAMP: u64 = 1_557_310_743;
const GENESIS_DIFFICULTY: u64 = 1000;
const EPOCH_REWARD: u64 = 125_000_000_000_000;
const CELLBASE_MATURITY: u64 = 0;
const ALERT_UNTIL_TIMESTAMP: u64 = 2_524_579_200;
const TARGET_HEIGHT: u64 = 1024;

thread_local! {
    // We store a cellbase for constructing a new transaction later
    static UNSPENT: RefCell<H256> = RefCell::new(H256::zero());
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
    let dao = genesis_dao_data(&always_success_tx).unwrap();
    let genesis = BlockBuilder::from_header_builder(
        HeaderBuilder::default()
            .timestamp(GENESIS_TIMESTAMP)
            .difficulty(U256::from(GENESIS_DIFFICULTY))
            .dao(dao),
    )
    .transaction(always_success_tx)
    .build();
    Consensus::default()
        .set_genesis_block(genesis)
        .set_epoch_reward(Capacity::shannons(EPOCH_REWARD))
        .set_cellbase_maturity(CELLBASE_MATURITY)
}

// Construct `Transaction` with an always-success cell
//
// The 1st transaction in genesis block, which contains a always_success_cell as the 1st output
fn always_success_transaction() -> Transaction {
    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let for_dep_group_input_cell = CellOutput::new(
        capacity_bytes!(1000),
        H256::zero(),
        always_success_script.clone(),
        None,
    );
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_cell_data.to_owned())
        .output(for_dep_group_input_cell)
        .output_data(Bytes::new())
        .witness(always_success_script.clone().into_witness())
        .build()
}

fn dep_group_transation() -> Transaction {
    let (_, _, always_success_script) = always_success_cell();
    let always_success_tx = always_success_transaction();
    let dep = CellDep::Cell(OutPoint::new(always_success_tx.hash().clone(), 0));
    let input = CellInput::new(OutPoint::new(always_success_tx.hash().clone(), 1), 0);
    let output_data =
        Bytes::from(OutPoint::new(always_success_tx.hash().clone(), 0).to_group_data());
    let output = CellOutputBuilder::from_data(&output_data)
        .capacity(capacity_bytes!(1000))
        .build();
    TransactionBuilder::default()
        .dep(dep)
        .input(input)
        .output(output)
        .output_data(output_data)
        .witness(always_success_script.clone().into_witness())
        .build()
}

// Construct the next block based the given `parent`
fn next_block(shared: &Shared, parent: &Header) -> Block {
    let epoch = {
        let last_epoch = shared
            .get_block_epoch(parent.hash())
            .expect("current epoch exists");
        shared
            .next_epoch_ext(&last_epoch, parent)
            .unwrap_or(last_epoch)
    };
    let (_, reward) = shared.finalize_block_reward(parent).unwrap();
    let cellbase = always_success_cellbase(parent.number() + 1, reward.total);

    let mut transactions = vec![cellbase];
    let mut proposals = Vec::new();
    // We store a cellbase for constructing a new transaction later
    if parent.number() == 0 {
        UNSPENT.with(|unspent| {
            *unspent.borrow_mut() = transactions[0].hash().to_owned();
        });
        proposals.push(dep_group_transation().proposal_short_id());
    }

    if parent.number() == 2 {
        transactions.push(dep_group_transation());
    }

    let dao = {
        let chain_state = shared.lock_chain_state();
        let resolved_txs = transactions
            .iter()
            .map(|tx| {
                resolve_transaction(tx, &mut Default::default(), &*chain_state, &*chain_state)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        DaoCalculator::new(shared.consensus(), shared.store())
            .dao_field(&resolved_txs, parent)
            .unwrap()
    };
    BlockBuilder::default()
        .transactions(transactions)
        .proposals(proposals)
        .header_builder(
            HeaderBuilder::default()
                .parent_hash(parent.hash().to_owned())
                .number(parent.number() + 1)
                .epoch(epoch.number())
                .timestamp(parent.timestamp() + 1)
                .difficulty(epoch.difficulty().clone())
                .dao(dao),
        )
        .build()
}

// Setup the running environment
fn setup_node(height: u64) -> (Shared, ChainController, RpcServer) {
    let shared = SharedBuilder::default()
        .consensus(always_success_consensus())
        .build()
        .unwrap();
    let chain_controller = {
        let notify = NotifyService::default().start::<&str>(None);
        ChainService::new(shared.clone(), notify).start::<&str>(None)
    };

    // Build chain, insert [1, height) blocks
    let mut parent = always_success_consensus().genesis_block;
    for _ in 0..height {
        let block = next_block(&shared, parent.header());
        chain_controller
            .process_block(Arc::new(block.clone()), true)
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
            shared.consensus().identify_name(),
            "0.1.0".to_string(),
        )
        .start::<&str>(Default::default(), None)
        .expect("Start network service failed")
    };
    let synchronizer = {
        let sync_shared_state = Arc::new(SyncSharedState::new(shared.clone()));
        Synchronizer::new(chain_controller.clone(), Arc::clone(&sync_shared_state))
    };
    let indexer_store = {
        let db_config = DBConfig {
            path: dir.join("indexer"),
            ..Default::default()
        };
        let indexer_store = DefaultIndexerStore::new(&db_config, shared.clone());
        let (_, _, always_success_script) = always_success_cell();
        indexer_store.insert_lock_hash(&always_success_script.hash(), Some(0));
        // use hardcoded TXN_ATTACH_BLOCK_NUMS (100) value here to setup testing data.
        (0..=height / 100).for_each(|_| indexer_store.sync_index_states());
        indexer_store
    };
    let alert_notifier = {
        let alert_relayer = AlertRelayer::new(
            "0.1.0".to_string(),
            Default::default(),
            AlertSignatureConfig::default(),
        );
        let alert_notifier = alert_relayer.notifier();
        let alert = Arc::new(
            AlertBuilder::default()
                .id(42)
                .min_version(Some("0.0.1".into()))
                .max_version(Some("1.0.0".into()))
                .priority(1)
                .notice_until(ALERT_UNTIL_TIMESTAMP * 1000)
                .message("An example alert message!".into())
                .build(),
        );
        alert_notifier.lock().add(alert);
        Arc::clone(alert_notifier)
    };

    // Start rpc services
    let mut io = IoHandler::new();
    io.extend_with(
        ChainRpcImpl {
            shared: shared.clone(),
        }
        .to_delegate(),
    );
    io.extend_with(PoolRpcImpl::new(shared.clone(), network_controller.clone()).to_delegate());
    io.extend_with(
        NetworkRpcImpl {
            network_controller: network_controller.clone(),
        }
        .to_delegate(),
    );
    io.extend_with(
        StatsRpcImpl {
            shared: shared.clone(),
            synchronizer: synchronizer.clone(),
            alert_notifier,
        }
        .to_delegate(),
    );
    io.extend_with(
        IndexerRpcImpl {
            store: indexer_store,
        }
        .to_delegate(),
    );
    io.extend_with(
        ExperimentRpcImpl {
            shared: shared.clone(),
        }
        .to_delegate(),
    );
    let server = ServerBuilder::new(io)
        .cors(DomainsValidation::AllowOnly(vec![
            AccessControlAllowOrigin::Null,
            AccessControlAllowOrigin::Any,
        ]))
        .threads(1)
        .max_request_body_size(20_000_000)
        .start_http(&"127.0.0.1:0".parse().unwrap())
        .expect("JsonRpc initialize");
    let rpc_server = RpcServer { server };

    (shared, chain_controller, rpc_server)
}

fn load_cases_from_file() -> Vec<Value> {
    let mut file_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    file_path.push("json/rpc.json");
    let file = File::open(file_path).expect("opening test data json");
    let content: Value = from_reader(file).expect("reading test data json");
    content.as_array().expect("load in array format").clone()
}

// Construct a transaction which use tip-cellbase as input cell
fn construct_transaction(genesis_hash: &H256) -> Transaction {
    let previous_output = OutPoint::new(UNSPENT.with(|unspent| unspent.borrow().clone()), 0);
    let input = CellInput::new(previous_output, 0);
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(1000))
        .lock(always_success_cell().2.clone())
        .build();
    let dep1 = CellDep::Cell(OutPoint::new(
        always_success_transaction().hash().to_owned(),
        0,
    ));
    let dep2 = CellDep::CellWithHeader(
        OutPoint::new(always_success_transaction().hash().to_owned(), 0),
        genesis_hash.clone(),
    );
    let dep3 = CellDep::DepGroup(OutPoint::new(dep_group_transation().hash().to_owned(), 0));
    let dep4 = CellDep::Header(genesis_hash.clone());
    TransactionBuilder::default()
        .input(input)
        .output(output)
        .output_data(Bytes::new())
        .deps(vec![dep1, dep2, dep3, dep4])
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
    let request = request_of(method, params);
    match client
        .post(uri)
        .json(&request)
        .send()
        .expect("send request")
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
    let (tip, genesis_hash) = {
        let chain = shared.lock_chain_state();
        let tip = chain.tip_header().to_owned();
        let genesis_hash = chain.consensus().genesis_hash().clone();
        (tip, genesis_hash)
    };
    let tip_number = json!(tip.number().to_string());
    let tip_hash = json!(format!("{:#x}", tip.hash()));
    let (_, _, always_success_script) = always_success_cell();
    let always_success_script_hash = json!(format!("{:#x}", always_success_script.hash()));
    let always_success_out_point = {
        let out_point = OutPoint::new(always_success_transaction().hash().to_owned(), 0);
        let json_out_point: ckb_jsonrpc_types::OutPoint = out_point.into();
        json!(json_out_point)
    };
    let (transaction, transaction_hash) = {
        let transaction = construct_transaction(&genesis_hash);
        let transaction_hash = transaction.hash().to_owned();
        let json_transaction: ckb_jsonrpc_types::Transaction = (&transaction).into();
        (
            json!(json_transaction),
            json!(format!("{:#x}", transaction_hash)),
        )
    };
    let params = match method {
        "get_tip_block_number"
        | "get_tip_header"
        | "get_current_epoch"
        | "local_node_info"
        | "get_peers"
        | "get_banned_addresses"
        | "get_blockchain_info"
        | "tx_pool_info"
        | "get_peers_state"
        | "get_lock_hash_index_states" => vec![],
        "get_epoch_by_number" => vec![json!("0")],
        "get_block_hash" | "get_block_by_number" | "get_header_by_number" => vec![tip_number],
        "get_block" | "get_header" | "get_cellbase_output_capacity_details" => vec![tip_hash],
        "get_cells_by_lock_hash"
        | "get_live_cells_by_lock_hash"
        | "get_transactions_by_lock_hash" => {
            vec![always_success_script_hash, json!("0"), json!("2")]
        }
        "get_live_cell" => vec![always_success_out_point],
        "set_ban" => vec![
            json!("192.168.0.2"),
            json!("insert"),
            json!("1840546800000"),
            json!(true),
            json!("set_ban example"),
        ],
        "send_transaction" | "dry_run_transaction" | "_compute_transaction_hash" => {
            vec![transaction]
        }
        "get_transaction" => vec![transaction_hash],
        "index_lock_hash" => vec![
            json!(format!("{:#x}", always_success_script.hash())),
            json!("1024"),
        ],
        "deindex_lock_hash" => vec![json!(format!("{:#x}", always_success_script.hash()))],
        "_compute_code_hash" => vec![json!("0x123456")],
        "_compute_script_hash" => {
            let script = always_success_script.clone();
            let json_script: ckb_jsonrpc_types::Script = script.into();
            vec![json!(json_script)]
        }
        method => {
            panic!("Unknown method: {}", method);
        }
    };
    json!(params)
}

// Print the expected documentation based the actual results
fn print_document(shared: &Shared, client: &reqwest::Client, uri: &str) {
    let document: Vec<_> = load_cases_from_file()
        .iter_mut()
        .map(|case| {
            let method = case.get("method").expect("get method").as_str().unwrap();
            let params = params_of(shared, method);
            let result = if case.get("skip").unwrap_or(&json!(false)).as_bool().unwrap() {
                case.get("result").expect("get result").clone()
            } else {
                result_of(client, uri, method, params.clone())
            };

            let object = case.as_object_mut().unwrap();
            object.insert("params".to_string(), params);
            object.insert("result".to_string(), result);
            json!(object)
        })
        .collect();
    println!("\n\n###################################");
    println!("Generated RPC Documentation:");
    println!("{}", to_string_pretty(&document).unwrap());
    println!("###################################\n\n");
}

#[test]
fn test_rpc() {
    let (shared, _chain_controller, server) = setup_node(TARGET_HEIGHT);
    let client = reqwest::Client::new();
    let uri = format!(
        "http://{}:{}/",
        server.server.address().ip(),
        server.server.address().port()
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
            actual.push((
                method.clone(),
                case.get("params").expect("get params").clone(),
            ));
            expected.push((method.clone(), params_of(&shared, &method)));
        });
        if actual != expected {
            print_document(&shared, &client, &uri);
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
            actual.push((method.clone(), result));
        });
        if actual != expected {
            print_document(&shared, &client, &uri);
            pretty_assert_eq!(actual, expected, "Assert results of jsonrpc",);
        }
    }

    server.close();
}
