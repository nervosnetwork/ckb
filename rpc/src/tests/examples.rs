use super::{
    RpcTestRequest, RpcTestResponse, RpcTestSuite,
    setup::{always_success_consensus, setup_rpc_test_suite},
};
use crate::tests::always_success_transaction;
use ckb_test_chain_utils::always_success_cell;
use pretty_assertions::assert_eq as pretty_assert_eq;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::cmp;
use std::collections::BTreeSet;

use std::fs::{File, read_dir};
use std::hash;
use std::io::{self, BufRead};
use std::path::PathBuf;

use ckb_types::{
    H256,
    core::{Capacity, TransactionBuilder, TransactionView, capacity_bytes},
    h256,
    packed::{self, CellDep, CellInput, CellOutputBuilder, OutPoint},
    prelude::*,
};

const TARGET_HEIGHT: u64 = 1024;
const EXAMPLE_TX_PARENT: H256 =
    h256!("0x365698b50ca0da75dca2c87f9e7b563811d3b5813736b8cc62cc3b106faceb17");
const EXAMPLE_TX_HASH: H256 =
    h256!("0xa0ef4eb5f4ceeb08a4c8524d84c5da95dce2f608e0ca2ec8091191b0f330c6e3");

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
        panic!("Fail to parse the RPC method name from line: {line}");
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
        if let Some(request) = request {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Unexpected request. The request {request} has no matched response yet."),
            ));
        }

        let new_request: RpcTestRequest = serde_json::from_str(&code_block).map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Invalid JSONRPC Request: {e}\n{code_block}"),
            )
        })?;
        *request = Some(new_request);
    } else {
        let response: RpcTestResponse = serde_json::from_str(&code_block).map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Invalid JSONRPC Response: {e}\n{code_block}"),
            )
        })?;
        if let Some(request) = request.take() {
            if request.id != response.id {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Unmatched response id",
                ));
            }
            let request_display = format!("{request}");
            if !collected.insert(RpcTestExample { request, response }) {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("Duplicate example {request_display}"),
                ));
            }
        } else {
            return Err(io::Error::new(io::ErrorKind::Other, "Unexpected response"));
        }
    }

    Ok(())
}

fn construct_example_transaction() -> TransactionView {
    let previous_output = OutPoint::new(EXAMPLE_TX_PARENT.clone().into(), 0);
    let input = CellInput::new(previous_output, 0);
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100))
        .lock(always_success_cell().2.clone())
        .build();
    let cell_dep = CellDep::new_builder()
        .out_point(OutPoint::new(always_success_transaction().hash(), 0))
        .build();
    TransactionBuilder::default()
        .input(input)
        .output(output)
        .output_data(packed::Bytes::default())
        .cell_dep(cell_dep)
        .header_dep(always_success_consensus().genesis_hash())
        .build()
}

fn find_comment(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.starts_with("///") || line.starts_with("//!") {
        Some(line[3..].trim())
    } else {
        None
    }
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
            && path.file_stem().unwrap_or_default() != "indexer"
            && path.file_stem().unwrap_or_default() != "rich_indexer"
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
            example_tx.hash().into(),
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
    let suite = setup_rpc_test_suite(TARGET_HEIGHT, None);
    suite.send_example_transaction();
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
            assert!(result.is_ok(), "Deserialize response result error: {err}");
        }
    }
    *response = example.response.clone()
}

// * Use replace_rpc_response to skip the response matching assertions.
// * Fix timestamp related fields.
fn mock_rpc_response(example: &RpcTestExample, response: &mut RpcTestResponse) {
    use ckb_jsonrpc_types::{BannedAddr, Capacity, LocalNode, RemoteNode, Uint64};

    let example_tx_hash = format!("{EXAMPLE_TX_HASH:#x}");

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
        "get_transaction" => {
            response.result["time_added_to_pool"] =
                example.response.result["time_added_to_pool"].clone();
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
        "get_pool_tx_detail_info" => {
            response.result["timestamp"] = example.response.result["timestamp"].clone()
        }
        "estimate_fee_rate" => replace_rpc_response::<Uint64>(example, response),
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
                vec![json!(format!("{EXAMPLE_TX_HASH:#x}"))],
                example.request.params,
                "get_transaction(id=42) must query the example tx"
            );
        }
        ("generate_block", 42) => return false,
        ("generate_epochs", 42) => return false,
        ("get_fee_rate_statics", 42) => return false,
        ("get_fee_rate_statistics", 42) => return false,
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
