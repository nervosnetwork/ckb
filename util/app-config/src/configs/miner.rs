use ckb_jsonrpc_types::{JsonBytes, ScriptHashType};
use ckb_types::H256;
use serde::{Deserialize, Serialize};

/// TODO(doc): @doitian
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// TODO(doc): @doitian
    pub client: ClientConfig,
    /// TODO(doc): @doitian
    pub workers: Vec<WorkerConfig>,
}

/// TODO(doc): @doitian
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientConfig {
    /// TODO(doc): @doitian
    pub rpc_url: String,
    /// TODO(doc): @doitian
    pub poll_interval: u64,
    /// TODO(doc): @doitian
    pub block_on_submit: bool,
}

/// TODO(doc): @doitian
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "worker_type")]
pub enum WorkerConfig {
    /// TODO(doc): @doitian
    Dummy(DummyConfig),
    /// TODO(doc): @doitian
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

/// TODO(doc): @doitian
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "delay_type")]
pub enum DummyConfig {
    /// TODO(doc): @doitian
    Constant {
        /// TODO(doc): @doitian
        value: u64,
    },
    /// TODO(doc): @doitian
    Uniform {
        /// TODO(doc): @doitian
        low: u64,
        /// TODO(doc): @doitian
        high: u64,
    },
    /// TODO(doc): @doitian
    Normal {
        /// TODO(doc): @doitian
        mean: f64,
        /// TODO(doc): @doitian
        std_dev: f64,
    },
    /// TODO(doc): @doitian
    Poisson {
        /// TODO(doc): @doitian
        lambda: f64,
    },
}

/// TODO(doc): @doitian
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EaglesongSimpleConfig {
    /// TODO(doc): @doitian
    pub threads: usize,
    /// TODO(doc): @doitian
    #[serde(default)]
    pub extra_hash_function: Option<ExtraHashFunction>,
}

/// TODO(doc): @doitian
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ExtraHashFunction {
    /// TODO(doc): @doitian
    Blake2b,
}
