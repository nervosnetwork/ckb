use ckb_jsonrpc_types::Script;
use serde::{Deserialize, Serialize};

/// RPC modules.
#[derive(Clone, Debug, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum Module {
    Net,
    Chain,
    Miner,
    Pool,
    Experiment,
    Stats,
    IntegrationTest,
    Alert,
    Subscription,
    Debug,
}

/// RPC config options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// RPC server listen addresses.
    pub listen_address: String,
    /// RPC TCP server listen addresses.
    ///
    /// Only TCP and WS are supported to subscribe events via the Subscription RPC module.
    #[serde(default)]
    pub tcp_listen_address: Option<String>,
    /// RPC WS server listen addresses.
    ///
    /// Only TCP and WS are supported to subscribe events via the Subscription RPC module.
    #[serde(default)]
    pub ws_listen_address: Option<String>,
    /// Max request body size in bytes.
    pub max_request_body_size: usize,
    /// Number of RPC worker threads.
    pub threads: Option<usize>,
    /// Enabled RPC modules.
    pub modules: Vec<Module>,
    /// Rejects txs with scripts that might trigger known bugs
    #[serde(default)]
    pub reject_ill_transactions: bool,
    /// Whether enable deprecated RPC methods.
    ///
    /// Deprecated RPC methods are disabled by default.
    #[serde(default)]
    pub enable_deprecated_rpc: bool,
    /// Customized extra well known lock scripts.
    #[serde(default)]
    pub extra_well_known_lock_scripts: Vec<Script>,
    /// Customized extra well known type scripts.
    #[serde(default)]
    pub extra_well_known_type_scripts: Vec<Script>,
}

impl Config {
    /// Checks whether the Net module is enabled.
    pub fn net_enable(&self) -> bool {
        self.modules.contains(&Module::Net)
    }

    /// Checks whether the Chain module is enabled.
    pub fn chain_enable(&self) -> bool {
        self.modules.contains(&Module::Chain)
    }

    /// Checks whether the Miner module is enabled.
    pub fn miner_enable(&self) -> bool {
        self.modules.contains(&Module::Miner)
    }

    /// Checks whether the Pool module is enabled.
    pub fn pool_enable(&self) -> bool {
        self.modules.contains(&Module::Pool)
    }

    /// Checks whether the Experiment module is enabled.
    pub fn experiment_enable(&self) -> bool {
        self.modules.contains(&Module::Experiment)
    }

    /// Checks whether the Stats module is enabled.
    pub fn stats_enable(&self) -> bool {
        self.modules.contains(&Module::Stats)
    }

    /// Checks whether the Subscription module is enabled.
    pub fn subscription_enable(&self) -> bool {
        self.modules.contains(&Module::Subscription)
    }

    /// Checks whether the IntegrationTest module is enabled.
    pub fn integration_test_enable(&self) -> bool {
        self.modules.contains(&Module::IntegrationTest)
    }

    /// Checks whether the Alert module is enabled.
    pub fn alert_enable(&self) -> bool {
        self.modules.contains(&Module::Alert)
    }

    /// Checks whether the Debug module is enabled.
    pub fn debug_enable(&self) -> bool {
        self.modules.contains(&Module::Debug)
    }
}
