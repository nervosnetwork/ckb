use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Debug, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum Module {
    Net,
    Chain,
    Miner,
    Pool,
    Trace,
    Experiment,
    Stats,
    IntegrationTest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub listen_address: String,
    pub max_request_body_size: usize,
    pub threads: Option<usize>,
    pub modules: Vec<Module>,
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

    pub fn integration_test_enable(&self) -> bool {
        self.modules.contains(&Module::IntegrationTest)
    }
}
