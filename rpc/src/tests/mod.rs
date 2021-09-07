use crate::{RpcServer, ServiceBuilder};
use ckb_app_config::{BlockAssemblerConfig, NetworkConfig, RpcConfig, RpcModule};
use ckb_chain::chain::{ChainController, ChainService};
use ckb_dao::DaoCalculator;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_launcher::SharedBuilder;
use ckb_network::{DefaultExitHandler, NetworkService, NetworkState};
use ckb_shared::{Shared, Snapshot};
use ckb_store::ChainStore;
use ckb_test_chain_utils::{
    always_success_cell, always_success_cellbase, always_success_consensus,
};
use ckb_types::{
    core::{
        cell::resolve_transaction, BlockBuilder, BlockView, FeeRate, HeaderView,
        TransactionBuilder, TransactionView,
    },
    h256,
    packed::{CellInput, OutPoint},
    prelude::*,
    H256,
};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, fmt, sync::Arc};

mod error;
mod examples;
mod module;

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

#[allow(dead_code)]
struct RpcTestSuite {
    rpc_client: reqwest::blocking::Client,
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
}

// Construct the next block based the given `parent`
fn next_block(shared: &Shared, parent: &HeaderView) -> BlockView {
    let snapshot: &Snapshot = &shared.snapshot();
    let epoch = shared
        .consensus()
        .next_epoch_ext(parent, &snapshot.as_data_provider())
        .unwrap()
        .epoch();
    let (_, reward) = snapshot.finalize_block_reward(parent).unwrap();
    let cellbase = always_success_cellbase(parent.number() + 1, reward.total, shared.consensus());

    let dao = {
        let resolved_cellbase =
            resolve_transaction(cellbase.clone(), &mut HashSet::new(), snapshot, snapshot).unwrap();
        let data_loader = shared.store().as_data_provider();
        DaoCalculator::new(shared.consensus(), &data_loader)
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

// setup a chain with 20 blocks and enable `Chain`, `Miner` and `Pool` rpc modules for unit test
// there is a similar fn `setup_rpc_test_suite` which enables all rpc modules, may be refactored into one fn with different paramsters in other PRs
fn setup() -> RpcTestSuite {
    let (shared, mut pack) = SharedBuilder::with_temp_db()
        .consensus(always_success_consensus())
        .block_assembler_config(Some(BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: Default::default(),
            hash_type: ScriptHashType::Data,
            message: Default::default(),
            use_binary_version_as_message_prefix: false,
            binary_version: "TEST".to_string(),
        }))
        .build()
        .unwrap();
    let chain_controller =
        ChainService::new(shared.clone(), pack.take_proposal_table()).start::<&str>(None);

    // Start network services
    let dir = tempfile::tempdir()
        .expect("create tempdir failed")
        .path()
        .to_path_buf();
    let network_controller = {
        let network_config = NetworkConfig {
            path: dir,
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

    // Build chain, insert 20 blocks
    let mut parent = shared.consensus().genesis_block().clone();

    for _ in 0..20 {
        let block = next_block(&shared, &parent.header());
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("processing new block should be ok");
        parent = block;
    }

    // Start rpc services
    let rpc_config = RpcConfig {
        listen_address: "127.0.0.1:0".to_owned(),
        tcp_listen_address: None,
        ws_listen_address: None,
        max_request_body_size: 20_000_000,
        threads: None,
        modules: vec![
            RpcModule::Chain,
            RpcModule::Miner,
            RpcModule::Pool,
            RpcModule::IntegrationTest,
        ],
        reject_ill_transactions: false,
        enable_deprecated_rpc: false,
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
        .enable_integration_test(shared.clone(), network_controller, chain_controller.clone());
    let io_handler = builder.build();

    let rpc_server = RpcServer::new(rpc_config, io_handler, shared.notify_controller());
    let rpc_uri = format!(
        "http://{}:{}/",
        rpc_server.http_address().ip(),
        rpc_server.http_address().port()
    );
    let rpc_client = Client::new();
    RpcTestSuite {
        shared,
        chain_controller,
        rpc_server,
        rpc_uri,
        rpc_client,
    }
}
