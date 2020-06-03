use ckb_jsonrpc_types::{JsonBytes, ScriptHashType};
use ckb_types::H256;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "delay_type")]
pub enum DummyConfig {
    Constant { value: u64 },
    Uniform { low: u64, high: u64 },
    Normal { mean: f64, std_dev: f64 },
    Poisson { lambda: f64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EaglesongSimpleConfig {
    pub threads: usize,
    #[serde(default)]
    pub extra_hash_function: Option<ExtraHashFunction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ExtraHashFunction {
    Blake2b,
}
