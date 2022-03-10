use crate::{
    tests::{always_success_transaction, next_block},
    RpcServer, ServiceBuilder,
};
use ckb_app_config::{
    BlockAssemblerConfig, NetworkAlertConfig, NetworkConfig, RpcConfig, RpcModule,
};
use ckb_chain::chain::ChainService;
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_dao_utils::genesis_dao_data;
use ckb_launcher::SharedBuilder;
use ckb_network::{DefaultExitHandler, NetworkService, NetworkState};
use ckb_network_alert::alert_relayer::AlertRelayer;
use ckb_notify::NotifyService;
use ckb_sync::SyncShared;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    core::{
        capacity_bytes, BlockBuilder, Capacity, EpochNumberWithFraction, FeeRate,
        TransactionBuilder, TransactionView,
    },
    h256,
    packed::{AlertBuilder, CellDep, CellInput, CellOutputBuilder, OutPoint, RawAlertBuilder},
    prelude::*,
    H256,
};
use pretty_assertions::assert_eq as pretty_assert_eq;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::cmp;
use std::collections::BTreeSet;
use std::fs::{read_dir, File};
use std::hash;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::Arc;
use std::{thread::sleep, time::Duration};

use super::{RpcTestRequest, RpcTestResponse, RpcTestSuite};

const GENESIS_TIMESTAMP: u64 = 1_557_310_743;
const GENESIS_TARGET: u32 = 0x2001_0000;
const EPOCH_REWARD: u64 = 125_000_000_000_000;
const CELLBASE_MATURITY: u64 = 0;
const ALERT_UNTIL_TIMESTAMP: u64 = 2_524_579_200;
const TARGET_HEIGHT: u64 = 1024;
const EXAMPLE_TX_PARENT: H256 =
    h256!("0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17");
const EXAMPLE_TX_HASH: H256 =
    h256!("0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3");

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
        .epoch(EpochNumberWithFraction::new_unchecked(0, 0, 0).pack())
        .dao(dao)
        .transaction(always_success_tx)
        .build();
    ConsensusBuilder::default()
        .genesis_block(genesis)
        .initial_primary_epoch_reward(Capacity::shannons(EPOCH_REWARD))
        .cellbase_maturity(EpochNumberWithFraction::from_full_value(CELLBASE_MATURITY))
        .build()
}

fn construct_example_transaction() -> TransactionView {
    let previous_output = OutPoint::new(EXAMPLE_TX_PARENT.clone().pack(), 0);
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

fn json_bytes(hex: &str) -> ckb_jsonrpc_types::JsonBytes {
    serde_json::from_value(json!(hex)).expect("JsonBytes")
}

// Setup the running environment
fn setup_rpc_test_suite(height: u64) -> RpcTestSuite {
    let (shared, mut pack) = SharedBuilder::with_temp_db()
        .consensus(always_success_consensus())
        .block_assembler_config(Some(BlockAssemblerConfig {
            code_hash: h256!("0x1892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df2"),
            hash_type: ckb_jsonrpc_types::ScriptHashType::Type,
            args: json_bytes("0xb2e61ff569acf041b3c2c17724e2379c581eeac3"),
            message: "message".pack().into(),
            use_binary_version_as_message_prefix: true,
            binary_version: "TEST".to_string(),
            update_interval_millis: 800,
        }))
        .build()
        .unwrap();
    let chain_controller =
        ChainService::new(shared.clone(), pack.take_proposal_table()).start::<&str>(None);

    // Start network services
    let temp_dir = tempfile::tempdir().expect("create tempdir failed");

    let temp_path = temp_dir.path().to_path_buf();
    let network_controller = {
        let network_config = NetworkConfig {
            path: temp_path,
            ping_interval_secs: 1,
            ping_timeout_secs: 1,
            connect_outbound_interval_secs: 1,
            ..Default::default()
        };
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
        .start(shared.async_handle())
        .expect("Start network service failed")
    };

    pack.take_tx_pool_builder()
        .start(network_controller.clone());

    let tx_pool = shared.tx_pool_controller();
    while !tx_pool.service_started() {
        sleep(Duration::from_millis(400));
    }

    // Build chain, insert [1, height) blocks
    let mut parent = always_success_consensus().genesis_block;

    for _ in 0..height {
        let block = next_block(&shared, &parent.header());
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("processing new block should be ok");
        parent = block;
    }
    assert_eq!(
        EXAMPLE_TX_PARENT,
        parent.tx_hashes()[0].unpack(),
        "Expect the last cellbase tx hash matches the constant, which is used later in an example tx."
    );

    let sync_shared = Arc::new(SyncShared::new(
        shared.clone(),
        Default::default(),
        pack.take_relay_tx_receiver(),
    ));

    let notify_controller = NotifyService::new(Default::default()).start(Some("test"));
    let (alert_notifier, alert_verifier) = {
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
        (
            Arc::clone(alert_notifier),
            Arc::clone(alert_relayer.verifier()),
        )
    };

    // Start rpc services
    let rpc_config = RpcConfig {
        listen_address: "127.0.0.1:0".to_owned(),
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
            RpcModule::IntegrationTest,
            RpcModule::Alert,
            RpcModule::Subscription,
            RpcModule::Debug,
        ],
        reject_ill_transactions: true,
        // enable deprecated rpc in unit test
        enable_deprecated_rpc: true,
        extra_well_known_lock_scripts: vec![],
        extra_well_known_type_scripts: vec![],
    };

    let builder = ServiceBuilder::new(&rpc_config)
        .enable_chain(shared.clone())
        .enable_pool(shared.clone(), FeeRate::zero(), true, vec![], vec![])
        .enable_miner(
            shared.clone(),
            network_controller.clone(),
            chain_controller.clone(),
            true,
        )
        .enable_net(network_controller.clone(), sync_shared)
        .enable_stats(shared.clone(), Arc::clone(&alert_notifier))
        .enable_experiment(shared.clone())
        .enable_integration_test(
            shared.clone(),
            network_controller.clone(),
            chain_controller.clone(),
        )
        .enable_debug()
        .enable_alert(alert_verifier, alert_notifier, network_controller);
    let io_handler = builder.build();

    let rpc_server = RpcServer::new(rpc_config, io_handler, shared.notify_controller());
    let rpc_uri = format!(
        "http://{}:{}/",
        rpc_server.http_address().ip(),
        rpc_server.http_address().port()
    );
    let rpc_client = reqwest::blocking::Client::new();

    let suite = RpcTestSuite {
        shared,
        chain_controller: chain_controller.clone(),
        rpc_server,
        rpc_uri,
        rpc_client,
        _tmp_dir: temp_dir,
    };

    suite.wait_block_template_number(height + 1);

    // insert a fork block for rpc `get_fork_block` test
    {
        let fork_block = parent
            .as_advanced_builder()
            .header(
                parent
                    .header()
                    .as_advanced_builder()
                    .timestamp((parent.header().timestamp() + 1).pack())
                    .build(),
            )
            .build();
        chain_controller
            .process_block(Arc::new(fork_block))
            .expect("processing new block should be ok");
    }

    suite.send_example_transaction();

    suite
}

fn find_comment(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.starts_with("///") || line.starts_with("//!") {
        Some(line[3..].trim())
    } else {
        None
    }
}

fn find_rpc_method(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.starts_with("#[")
        && line.contains("rpc")
        && line.contains("name")
        && !line.contains("noexample")
    {
        for w in line.split('=').collect::<Vec<_>>().windows(2) {
            if w[0].trim().ends_with("name") && w[1].trim().starts_with('"') {
                let name = w[1].split('"').collect::<Vec<_>>()[1];
                if let Some(n) = name.strip_prefix("deprecated.") {
                    return Some(n);
                } else {
                    return Some(name);
                }
            }
        }
        panic!("Fail to parse the RPC method name from line: {}", line);
    } else {
        None
    }
}

fn collect_code_block(
    collected: &mut BTreeSet<RpcTestExample>,
    request: &mut Option<RpcTestRequest>,
    code_block: String,
) -> io::Result<()> {
    if code_block.contains("\"method\":") {
        if let Some(ref request) = request {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "Unexpected request. The request {} has no matched response yet.",
                    request
                ),
            ));
        }

        let new_request: RpcTestRequest = serde_json::from_str(&code_block).map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Invalid JSONRPC Request: {}\n{}", e, code_block),
            )
        })?;
        *request = Some(new_request);
    } else {
        let response: RpcTestResponse = serde_json::from_str(&code_block).map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Invalid JSONRPC Response: {}\n{}", e, code_block),
            )
        })?;
        if let Some(request) = request.take() {
            if request.id != response.id {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Unmatched response id",
                ));
            }
            let request_display = format!("{}", request);
            if !collected.insert(RpcTestExample { request, response }) {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("Duplicate example {}", request_display),
                ));
            }
        } else {
            return Err(io::Error::new(io::ErrorKind::Other, "Unexpected response"));
        }
    }

    Ok(())
}

fn collect_rpc_examples_in_file(
    collected: &mut BTreeSet<RpcTestExample>,
    path: PathBuf,
) -> io::Result<()> {
    let reader = io::BufReader::new(File::open(&path)?);

    let mut collecting = Vec::new();
    let mut request: Option<RpcTestRequest> = None;

    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        if let Some(comment) = find_comment(&line) {
            if comment == "```json" {
                if !collecting.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("{}:{}: Unexpected code block start", path.display(), lineno),
                    ));
                }
                collecting.push("".to_string());
            } else if comment == "```" {
                let code_block = collecting.join("\n");
                if code_block.contains("\"jsonrpc\":") {
                    collect_code_block(collected, &mut request, code_block).map_err(|e| {
                        io::Error::new(
                            io::ErrorKind::Other,
                            format!("{}:{}: {}", path.display(), lineno, e),
                        )
                    })?;
                }
                collecting.clear();
            } else if !collecting.is_empty() {
                collecting.push(comment.to_string());
            }
        } else {
            if !collecting.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("{}:{}: Unexpected end of comment", path.display(), lineno),
                ));
            }

            if let Some(rpc_method) = find_rpc_method(&line) {
                let key = RpcTestExample::search(rpc_method.to_string(), 42);
                assert!(
                    collected.contains(&key),
                    "{}:{}: Expect an example with id=42 for RPC method {}. \
                    To skip the test, add a comment \"noexample\" after #[rpc]",
                    path.display(),
                    lineno,
                    rpc_method,
                );
            }
        }
    }

    if collecting.is_empty() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "{}: Unexpected EOF while the code block is still open",
                path.display()
            ),
        ))
    }
}

// Use HashSet to randomize the order
fn collect_rpc_examples() -> io::Result<BTreeSet<RpcTestExample>> {
    let mut modules_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    modules_dir.push("src");
    modules_dir.push("module");

    let mut examples = BTreeSet::new();

    for module_file in read_dir(modules_dir)? {
        let path = module_file?.path();
        if path.extension().unwrap_or_default() == "rs"
            && path.file_stem().unwrap_or_default() != "mod"
            && path.file_stem().unwrap_or_default() != "debug"
        {
            collect_rpc_examples_in_file(&mut examples, path)?;
        }
    }

    Ok(examples)
}

#[derive(Debug, Clone)]
struct RpcTestExample {
    request: RpcTestRequest,
    response: RpcTestResponse,
}

impl hash::Hash for RpcTestExample {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.request.method.hash(state);
        self.request.id.hash(state);
    }
}

impl PartialEq for RpcTestExample {
    fn eq(&self, other: &Self) -> bool {
        self.request.method == other.request.method && self.request.id == other.request.id
    }
}

impl Eq for RpcTestExample {}

impl Ord for RpcTestExample {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.request.cmp(&other.request)
    }
}

impl PartialOrd for RpcTestExample {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl RpcTestExample {
    fn search(method: String, id: usize) -> Self {
        RpcTestExample {
            request: RpcTestRequest {
                id,
                method,
                jsonrpc: "2.0".to_string(),
                params: vec![],
            },
            response: RpcTestResponse {
                id,
                jsonrpc: "2.0".to_string(),
                result: Value::Null,
                error: Value::Null,
            },
        }
    }
}

impl RpcTestSuite {
    fn send_example_transaction(&self) {
        let example_tx = construct_example_transaction();
        assert_eq!(
            EXAMPLE_TX_HASH,
            example_tx.hash().unpack(),
            "Expect the example tx hash match the constant"
        );
        let example_tx: ckb_jsonrpc_types::Transaction = example_tx.data().into();
        self.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "send_transaction".to_string(),
            params: vec![json!(example_tx), json!("passthrough")],
        });
    }

    fn wait_block_template_update(&self) {
        self.wait_block_template_array_ge("proposals", 1)
    }

    fn run_example(&self, example: &RpcTestExample) {
        let mut actual = self.rpc(&example.request);
        mock_rpc_response(example, &mut actual);
        pretty_assert_eq!(
            example.response,
            actual,
            "RPC Example {} got the following unexpected response:\n\n{}",
            example.request,
            actual.json(),
        );
    }
}

/// ## RPC Examples Test FAQ
///
/// Q. How to add tests?
///
/// Test cases are collected from code comments. Please put request and response JSON in their own code
/// blocks and set the fenced code block type to "json".
///
/// The first example must use id 42. And extra examples for the same method must use different
/// ids.
///
/// Q. How to skip an RPC example test
///
/// Add noexample in the comment in the same line with `#[rpc(name = "...")]`.
///
/// Q. How to setup and teardown the test case?
///
/// Edit `around_rpc_example`.
#[test]
fn test_rpc_examples() {
    let suite = setup_rpc_test_suite(TARGET_HEIGHT);
    for example in collect_rpc_examples().expect("collect RPC examples") {
        println!("Test RPC Example {}", example.request);
        around_rpc_example(&suite, example);
    }
}

fn replace_rpc_response<T>(example: &RpcTestExample, response: &mut RpcTestResponse)
where
    T: DeserializeOwned,
{
    if !example.response.result.is_null() {
        let result: serde_json::Result<T> = serde_json::from_value(example.response.result.clone());
        if let Err(ref err) = result {
            assert!(result.is_ok(), "Deserialize response result error: {}", err);
        }
    }
    *response = example.response.clone()
}

// * Use replace_rpc_response to skip the response matching assertions.
// * Fix timestamp related fields.
fn mock_rpc_response(example: &RpcTestExample, response: &mut RpcTestResponse) {
    use ckb_jsonrpc_types::{BannedAddr, Capacity, LocalNode, RemoteNode, Uint64};

    let example_tx_hash = format!("{:#x}", EXAMPLE_TX_HASH);

    match example.request.method.as_str() {
        "local_node_info" => replace_rpc_response::<LocalNode>(example, response),
        "get_peers" => replace_rpc_response::<Vec<RemoteNode>>(example, response),
        "get_banned_addresses" => replace_rpc_response::<Vec<BannedAddr>>(example, response),
        "calculate_dao_maximum_withdraw" => replace_rpc_response::<Capacity>(example, response),
        "subscribe" => replace_rpc_response::<Uint64>(example, response),
        "unsubscribe" => replace_rpc_response::<bool>(example, response),
        "send_transaction" => replace_rpc_response::<H256>(example, response),
        "get_block_template" => {
            response.result["current_time"] = example.response.result["current_time"].clone();
            response.result["work_id"] = example.response.result["work_id"].clone();
        }
        "tx_pool_info" => {
            response.result["last_txs_updated_at"] =
                example.response.result["last_txs_updated_at"].clone()
        }
        "get_blockchain_info" => {
            response.result["chain"] = example.response.result["chain"].clone()
        }
        "send_alert" => response.error["data"] = example.response.error["data"].clone(),
        "get_raw_tx_pool" => {
            response.result["pending"][example_tx_hash.as_str()]["timestamp"] =
                example.response.result["pending"][example_tx_hash.as_str()]["timestamp"].clone()
        }
        "generate_block_with_template" => replace_rpc_response::<H256>(example, response),
        "generate_block" => replace_rpc_response::<H256>(example, response),
        "process_block_without_verify" => replace_rpc_response::<H256>(example, response),
        "notify_transaction" => replace_rpc_response::<H256>(example, response),
        _ => {}
    }
}

// Sets up RPC example test.
//
// Returns false to skip the example.
fn before_rpc_example(suite: &RpcTestSuite, example: &mut RpcTestExample) -> bool {
    match (example.request.method.as_str(), example.request.id) {
        ("get_transaction", 42) => {
            assert_eq!(
                vec![json!(format!("{:#x}", EXAMPLE_TX_HASH))],
                example.request.params,
                "get_transaction(id=42) must query the example tx"
            );
        }
        ("generate_block", 42) => return false,
        ("generate_block_with_template", 42) => return false,
        ("process_block_without_verify", 42) => return false,
        ("notify_transaction", 42) => return false,
        ("truncate", 42) => return false,
        ("get_block_template", 42) => suite.wait_block_template_update(),
        _ => return true,
    }

    true
}

// Tears down RPC example test.
fn after_rpc_example(suite: &RpcTestSuite, example: &RpcTestExample) {
    match example.request.method.as_str() {
        "clear_tx_pool" => suite.send_example_transaction(),
        "send_transaction" => {
            suite.rpc(&RpcTestRequest {
                id: 42,
                jsonrpc: "2.0".to_string(),
                method: "clear_tx_pool".to_string(),
                params: vec![],
            });
            suite.send_example_transaction()
        }
        "remove_transaction" => suite.send_example_transaction(),
        _ => {}
    }
}

// Use mock_rpc_response, before_rpc_example and after_rpc_example to tweak the test examples.
//
// Please ensure that examples do not depend on the execution sequence. Use `before_rpc_example`
// and `after_rpc_example` to prepare the test environment and rollback the environment.
fn around_rpc_example(suite: &RpcTestSuite, mut example: RpcTestExample) {
    if !before_rpc_example(suite, &mut example) {
        return;
    }

    suite.run_example(&example);

    after_rpc_example(suite, &example);
}
