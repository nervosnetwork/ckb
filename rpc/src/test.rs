use crate::{RpcServer, ServiceBuilder};
use ckb_app_config::{
    BlockAssemblerConfig, IndexerConfig, NetworkAlertConfig, NetworkConfig, RpcConfig, RpcModule,
};
use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_dao::DaoCalculator;
use ckb_dao_utils::genesis_dao_data;
use ckb_fee_estimator::FeeRate;
use ckb_indexer::{DefaultIndexerStore, IndexerStore};
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
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fmt;
use std::fs::{read_dir, File};
use std::hash;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::Arc;

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

fn json_bytes(hex: &str) -> ckb_jsonrpc_types::JsonBytes {
    serde_json::from_value(json!(hex)).expect("JsonBytes")
}

// Setup the running environment
fn setup_rpc_test_suite(height: u64) -> RpcTestSuite {
    let (shared, table) = SharedBuilder::default()
        .consensus(always_success_consensus())
        .block_assembler_config(Some(BlockAssemblerConfig {
            code_hash: h256!("0x1892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df2"),
            hash_type: ckb_jsonrpc_types::ScriptHashType::Type,
            args: json_bytes("0xb2e61ff569acf041b3c2c17724e2379c581eeac3"),
            message: Default::default(),
        }))
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
    assert_eq!(
        EXAMPLE_TX_PARENT,
        parent.tx_hashes()[0].unpack(),
        "Expect the last cellbase tx hash matches the constant, which is used later in an example tx."
    );

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
        .enable_integration_test(
            shared.clone(),
            network_controller.clone(),
            chain_controller.clone(),
        )
        .enable_indexer(&indexer_config, shared.clone())
        .enable_debug()
        .enable_alert(alert_verifier, alert_notifier, network_controller);
    let io_handler = builder.build();

    let rpc_server = RpcServer::new(rpc_config, io_handler, shared.notify_controller());
    let rpc_uri = format!(
        "http://{}:{}/",
        rpc_server.http_address().ip(),
        rpc_server.http_address().port()
    );
    let rpc_client = reqwest::Client::new();

    let suite = RpcTestSuite {
        shared,
        chain_controller,
        rpc_server,
        rpc_uri,
        rpc_client,
    };

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
                if name.starts_with("deprecated.") {
                    return Some(&name["deprecated.".len()..]);
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
    collected: &mut HashSet<RpcTestExample>,
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
    collected: &mut HashSet<RpcTestExample>,
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
fn collect_rpc_examples() -> io::Result<HashSet<RpcTestExample>> {
    let mut modules_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    modules_dir.push("src");
    modules_dir.push("module");

    let mut examples = HashSet::new();

    for module_file in read_dir(modules_dir)? {
        let path = module_file?.path();
        if path.extension().unwrap_or_default() == "rs"
            && path.file_stem().unwrap_or_default() != "mod"
            && path.file_stem().unwrap_or_default() != "test"
            && path.file_stem().unwrap_or_default() != "debug"
        {
            collect_rpc_examples_in_file(&mut examples, path)?;
        }
    }

    Ok(examples)
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
struct RpcTestRequest {
    pub id: usize,
    pub jsonrpc: String,
    pub method: String,
    pub params: Vec<Value>,
}

impl fmt::Display for RpcTestRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}(id={})", self.method, self.id)
    }
}

impl RpcTestRequest {
    fn json(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Default)]
struct RpcTestResponse {
    pub id: usize,
    pub jsonrpc: String,
    #[serde(default)]
    pub result: Value,
    #[serde(default)]
    pub error: Value,
}

impl RpcTestResponse {
    fn json(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

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

#[allow(dead_code)]
struct RpcTestSuite {
    rpc_client: reqwest::Client,
    rpc_uri: String,
    shared: Shared,
    chain_controller: ChainController,
    rpc_server: RpcServer,
}

impl RpcTestSuite {
    fn rpc(&self, request: &RpcTestRequest) -> RpcTestResponse {
        self.rpc_client
            .post(&self.rpc_uri)
            .json(&request)
            .send()
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to call RPC request: {:?}\n\nrequest = {:?}",
                    e,
                    request.json(),
                )
            })
            .json::<RpcTestResponse>()
            .expect("Deserialize RpcTestRequest")
    }

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

    fn run_example(&self, example: &RpcTestExample) {
        let mut actual = self.rpc(&example.request);
        mock_rpc_response(&example, &mut actual);
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
    use ckb_jsonrpc_types::{BannedAddr, Capacity, LocalNode, PeerState, RemoteNode, Uint64};

    match example.request.method.as_str() {
        "local_node_info" => replace_rpc_response::<LocalNode>(example, response),
        "get_peers" => replace_rpc_response::<Vec<RemoteNode>>(example, response),
        "get_peers_state" => replace_rpc_response::<Vec<PeerState>>(example, response),
        "get_banned_addresses" => replace_rpc_response::<Vec<BannedAddr>>(example, response),
        "calculate_dao_maximum_withdraw" => replace_rpc_response::<Capacity>(example, response),
        "subscribe" => replace_rpc_response::<Uint64>(example, response),
        "unsubscribe" => replace_rpc_response::<bool>(example, response),
        "send_transaction" => replace_rpc_response::<H256>(example, response),
        "get_block_template" => {
            response.result["current_time"] = example.response.result["current_time"].clone()
        }
        "get_blockchain_info" => {
            response.result["chain"] = example.response.result["chain"].clone()
        }
        "send_alert" => response.error["data"] = example.response.error["data"].clone(),
        _ => {}
    }
}

// Sets up RPC example test.
//
// Returns false to skip the example.
fn before_rpc_example(_suite: &RpcTestSuite, example: &mut RpcTestExample) -> bool {
    match (example.request.method.as_str(), example.request.id) {
        ("get_transaction", 42) => {
            assert_eq!(
                vec![json!(format!("{:#x}", EXAMPLE_TX_HASH))],
                example.request.params,
                "get_transaction(id=42) must query the example tx"
            );
        }
        ("deindex_lock_hash", _) => {
            let (_, _, always_success_script) = always_success_cell();
            let alway_success_script_hash: H256 = always_success_script.calc_script_hash().unpack();
            assert_ne!(
                vec![json!(alway_success_script_hash)],
                example.request.params,
                "should not deindex the example index"
            );
        }
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
