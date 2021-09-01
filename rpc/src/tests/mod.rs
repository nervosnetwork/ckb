use crate::RpcServer;
use ckb_chain::chain::ChainController;
use ckb_dao::DaoCalculator;
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
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, fmt};

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
