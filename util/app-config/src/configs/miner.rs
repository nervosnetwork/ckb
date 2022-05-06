use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// Miner config options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// RPC client config options.
    ///
    /// Miner connects to CKB node via RPC.
    pub client: ClientConfig,
    /// Miner workers config options.
    pub workers: Vec<WorkerConfig>,
}

/// RPC client config options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientConfig {
    /// CKB node RPC endpoint.
    pub rpc_url: String,
    /// The poll interval in seconds to get work from the CKB node.
    pub poll_interval: u64,
    /// By default, miner submits a block and continues to get the next work.
    ///
    /// When this is enabled, miner will block until the submission RPC returns.
    pub block_on_submit: bool,
    /// listen block_template notify instead of loop poll
    pub listen: Option<SocketAddr>,
}

/// Miner worker config options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "worker_type")]
pub enum WorkerConfig {
    /// Dummy worker which submits an arbitrary answer.
    Dummy(DummyConfig),
    /// Eaglesong worker which solves Eaglesong PoW.
    EaglesongSimple(EaglesongSimpleConfig),
}

/// Dummy worker config options.
///
/// Dummy worker can submit the new block at any time. This controls the pace that how much time
/// the worker must wait before submitting a new block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "delay_type")]
pub enum DummyConfig {
    /// Waits for a constant delay.
    Constant {
        /// The delay in seconds.
        value: u64,
    },
    /// Waits for a time which is uniformly sampled from a range.
    Uniform {
        /// The lower bound of the range (in seconds).
        low: u64,
        /// The upper bound of the range (in seconds).
        high: u64,
    },
    /// Picks the wait time from a normal distribution.
    Normal {
        /// The mean of the distribution (in seconds).
        mean: f64,
        /// The standard deviation.
        std_dev: f64,
    },
    /// Picks the wait time from a poisson distribution.
    Poisson {
        /// The parameter lambda of the poisson distribution.
        lambda: f64,
    },
}

/// Eaglesong worker config options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EaglesongSimpleConfig {
    /// Number of worker threads.
    pub threads: usize,
    /// Whether to perform an extra round of hash function on the Eaglesong output.
    #[serde(default)]
    pub extra_hash_function: Option<ExtraHashFunction>,
}

/// Specifies the hash function.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ExtraHashFunction {
    /// Blake2b hash with CKB preferences.
    Blake2b,
}
