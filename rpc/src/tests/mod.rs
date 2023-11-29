use ckb_chain::{start_chain_services, ChainController};
use ckb_chain_spec::consensus::Consensus;
use ckb_dao::DaoCalculator;
use ckb_reward_calculator::RewardCalculator;
use ckb_shared::{Shared, Snapshot};
use ckb_store::ChainStore;
use ckb_test_chain_utils::{always_success_cell, always_success_cellbase};
use ckb_types::{
    core::{
        cell::resolve_transaction, BlockBuilder, BlockView, HeaderView, TransactionBuilder,
        TransactionView,
    },
    packed::{CellInput, OutPoint},
    prelude::*,
};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::{cmp, collections::HashSet, fmt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use self::setup::setup_rpc_test_suite;

mod error;
mod examples;
mod fee_rate;
mod module;
mod setup;

#[derive(Debug, Deserialize, Serialize, Clone, Eq, PartialEq, Default)]
struct RpcTestRequest {
    pub id: usize,
    pub jsonrpc: String,
    pub method: String,
    pub params: Vec<Value>,
}

impl Ord for RpcTestRequest {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.method.cmp(&other.method)
    }
}

impl PartialOrd for RpcTestRequest {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
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

#[allow(dead_code)]
pub(crate) struct RpcTestSuite {
    rpc_client: Client,
    rpc_uri: String,
    tcp_uri: Option<String>,
    shared: Shared,
    chain_controller: ChainController,
    _tmp_dir: tempfile::TempDir,
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

    async fn tcp(&self, request: &RpcTestRequest) -> Result<RpcTestResponse, Box<dyn Error>> {
        // Connect to the server.
        assert!(self.tcp_uri.is_some());
        let mut stream = TcpStream::connect(self.tcp_uri.as_ref().unwrap()).await?;
        let json = serde_json::to_string(&request)? + "\n";
        stream.write_all(json.as_bytes()).await?;
        // Read the server's response.
        let mut buffer = [0; 1024];
        let n = stream.read(&mut buffer).await?;
        let response = std::str::from_utf8(&buffer[..n])?;
        let message: RpcTestResponse = serde_json::from_str(response)?;
        Ok(message)
    }

    fn wait_block_template_number(&self, target: u64) {
        use ckb_jsonrpc_types::Uint64;
        use std::{thread::sleep, time::Duration};

        let mut response = self.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "get_block_template".to_string(),
            params: vec![],
        });

        loop {
            let number: Uint64 = serde_json::from_value(response.result["number"].clone()).unwrap();
            if number.value() < target {
                sleep(Duration::from_millis(400));
                response = self.rpc(&RpcTestRequest {
                    id: 42,
                    jsonrpc: "2.0".to_string(),
                    method: "get_block_template".to_string(),
                    params: vec![],
                });
            } else {
                break;
            }
        }
    }

    fn wait_block_template_array_ge(&self, field: &str, size: usize) {
        use std::{thread::sleep, time::Duration};

        let mut response = self.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "get_block_template".to_string(),
            params: vec![],
        });

        loop {
            if response.result[field].as_array().unwrap().len() < size {
                sleep(Duration::from_millis(400));
                response = self.rpc(&RpcTestRequest {
                    id: 42,
                    jsonrpc: "2.0".to_string(),
                    method: "get_block_template".to_string(),
                    params: vec![],
                });
            } else {
                break;
            }
        }
    }
}

// Construct the next block based the given `parent`
fn next_block(shared: &Shared, parent: &HeaderView) -> BlockView {
    let snapshot: &Snapshot = &shared.snapshot();
    let epoch = shared
        .consensus()
        .next_epoch_ext(parent, &snapshot.borrow_as_data_loader())
        .unwrap()
        .epoch();
    let (_, reward) = RewardCalculator::new(snapshot.consensus(), snapshot)
        .block_reward_to_finalize(parent)
        .unwrap();
    let cellbase = always_success_cellbase(parent.number() + 1, reward.total, shared.consensus());

    let dao = {
        let resolved_cellbase =
            resolve_transaction(cellbase.clone(), &mut HashSet::new(), snapshot, snapshot).unwrap();
        let data_loader = shared.store().borrow_as_data_loader();
        DaoCalculator::new(shared.consensus(), &data_loader)
            .dao_field([resolved_cellbase].iter(), parent)
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

// setup a chain with 20 blocks and enable `Chain`, `Miner` and `Pool` rpc modules for unit test.
fn setup(consensus: Consensus) -> RpcTestSuite {
    setup_rpc_test_suite(20, Some(consensus))
}
