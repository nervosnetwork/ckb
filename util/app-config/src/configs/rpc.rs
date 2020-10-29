use serde::{Deserialize, Serialize};

/// TODO(doc): @doitian
#[derive(Clone, Debug, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum Module {
    /// TODO(doc): @doitian
    Net,
    /// TODO(doc): @doitian
    Chain,
    /// TODO(doc): @doitian
    Miner,
    /// TODO(doc): @doitian
    Pool,
    /// TODO(doc): @doitian
    Experiment,
    /// TODO(doc): @doitian
    Stats,
    /// TODO(doc): @doitian
    Indexer,
    /// TODO(doc): @doitian
    IntegrationTest,
    /// TODO(doc): @doitian
    Alert,
    /// TODO(doc): @doitian
    Subscription,
    /// TODO(doc): @doitian
    Debug,
}

/// TODO(doc): @doitian
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// TODO(doc): @doitian
    pub listen_address: String,
    /// TODO(doc): @doitian
    #[serde(default)]
    pub tcp_listen_address: Option<String>,
    /// TODO(doc): @doitian
    #[serde(default)]
    pub ws_listen_address: Option<String>,
    /// TODO(doc): @doitian
    pub max_request_body_size: usize,
    /// TODO(doc): @doitian
    pub threads: Option<usize>,
    /// TODO(doc): @doitian
    pub modules: Vec<Module>,
    /// Rejects txs with scripts that might trigger known bugs
    #[serde(default)]
    pub reject_ill_transactions: bool,
    /// TODO(doc): @doitian
    #[serde(default)]
    pub enable_deprecated_rpc: bool,
}

impl Config {
    /// TODO(doc): @doitian
    pub fn net_enable(&self) -> bool {
        self.modules.contains(&Module::Net)
    }

    /// TODO(doc): @doitian
    pub fn chain_enable(&self) -> bool {
        self.modules.contains(&Module::Chain)
    }

    /// TODO(doc): @doitian
    pub fn miner_enable(&self) -> bool {
        self.modules.contains(&Module::Miner)
    }

    /// TODO(doc): @doitian
    pub fn pool_enable(&self) -> bool {
        self.modules.contains(&Module::Pool)
    }

    /// TODO(doc): @doitian
    pub fn experiment_enable(&self) -> bool {
        self.modules.contains(&Module::Experiment)
    }

    /// TODO(doc): @doitian
    pub fn stats_enable(&self) -> bool {
        self.modules.contains(&Module::Stats)
    }

    /// TODO(doc): @doitian
    pub fn subscription_enable(&self) -> bool {
        self.modules.contains(&Module::Subscription)
    }

    /// TODO(doc): @doitian
    pub fn indexer_enable(&self) -> bool {
        self.modules.contains(&Module::Indexer)
    }

    /// TODO(doc): @doitian
    pub fn integration_test_enable(&self) -> bool {
        self.modules.contains(&Module::IntegrationTest)
    }

    /// TODO(doc): @doitian
    pub fn alert_enable(&self) -> bool {
        self.modules.contains(&Module::Alert)
    }

    /// TODO(doc): @doitian
    pub fn debug_enable(&self) -> bool {
        self.modules.contains(&Module::Debug)
    }
}
