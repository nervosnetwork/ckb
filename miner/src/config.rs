use jsonrpc_types::JsonBytes;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

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
pub struct WorkerConfig {
    pub worker_type: WorkerType,
    #[serde(flatten)]
    pub parameters: Option<HashMap<String, String>>,
}

impl WorkerConfig {
    pub fn get_value<T: FromStr>(&self, name: &str, default_value: T) -> T {
        self.parameters
            .as_ref()
            .and_then(|params| params.get(name).and_then(|value| value.parse::<T>().ok()))
            .unwrap_or_else(|| default_value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WorkerType {
    Dummy,
    CuckooSimple,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockAssemblerConfig {
    pub code_hash: H256,
    pub args: Vec<JsonBytes>,
}
