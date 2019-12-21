use crate::worker::{DummyConfig, EaglesongSimpleConfig};
use ckb_jsonrpc_types::{JsonBytes, ScriptHashType};
use ckb_types::H256;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MinerConfig {
    pub client: ClientConfig,
    pub workers: Vec<WorkerConfig>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientConfig {
    pub rpc_url: String,
    pub poll_interval: u64,
    pub block_on_submit: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "worker_type")]
pub enum WorkerConfig {
    Dummy(DummyConfig),
    EaglesongSimple(EaglesongSimpleConfig),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockAssemblerConfig {
    pub code_hash: H256,
    pub hash_type: ScriptHashType,
    pub args: Vec<JsonBytes>,
    #[serde(default)]
    pub message: JsonBytes,
}
