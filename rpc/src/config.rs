use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Debug, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum Module {
    Net,
    Chain,
    Miner,
    Pool,
    Trace,
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
    pub(crate) fn net_enable(&self) -> bool {
        self.modules.contains(&Module::Net)
    }

    pub(crate) fn chain_enable(&self) -> bool {
        self.modules.contains(&Module::Chain)
    }

    pub(crate) fn miner_enable(&self) -> bool {
        self.modules.contains(&Module::Miner)
    }

    pub(crate) fn pool_enable(&self) -> bool {
        self.modules.contains(&Module::Pool)
    }

    pub(crate) fn trace_enable(&self) -> bool {
        self.modules.contains(&Module::Trace)
    }

    pub(crate) fn integration_test_enable(&self) -> bool {
        self.modules.contains(&Module::IntegrationTest)
    }
}
