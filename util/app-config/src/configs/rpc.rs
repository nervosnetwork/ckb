use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum Module {
    Net,
    Chain,
    Miner,
    Pool,
    Experiment,
    Stats,
    Indexer,
    IntegrationTest,
    Alert,
    Subscription,
    Debug,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub listen_address: String,
    #[serde(default)]
    pub tcp_listen_address: Option<String>,
    #[serde(default)]
    pub ws_listen_address: Option<String>,
    pub max_request_body_size: usize,
    pub threads: Option<usize>,
    pub modules: Vec<Module>,
    // Rejects txs with scripts that might trigger known bugs
    #[serde(default)]
    pub reject_ill_transactions: bool,
    #[serde(default)]
    pub enable_deprecated_rpc: bool,
}

impl Config {
    pub fn net_enable(&self) -> bool {
        self.modules.contains(&Module::Net)
    }

    pub fn chain_enable(&self) -> bool {
        self.modules.contains(&Module::Chain)
    }

    pub fn miner_enable(&self) -> bool {
        self.modules.contains(&Module::Miner)
    }

    pub fn pool_enable(&self) -> bool {
        self.modules.contains(&Module::Pool)
    }

    pub fn experiment_enable(&self) -> bool {
        self.modules.contains(&Module::Experiment)
    }

    pub fn stats_enable(&self) -> bool {
        self.modules.contains(&Module::Stats)
    }

    pub fn subscription_enable(&self) -> bool {
        self.modules.contains(&Module::Subscription)
    }

    pub fn indexer_enable(&self) -> bool {
        self.modules.contains(&Module::Indexer)
    }

    pub fn integration_test_enable(&self) -> bool {
        self.modules.contains(&Module::IntegrationTest)
    }

    pub fn alert_enable(&self) -> bool {
        self.modules.contains(&Module::Alert)
    }

    pub fn debug_enable(&self) -> bool {
        self.modules.contains(&Module::Debug)
    }
}
