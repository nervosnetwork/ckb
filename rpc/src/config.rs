use serde_derive::Deserialize;
use std::path::PathBuf;

#[derive(Clone, Debug, Copy, Eq, PartialEq, Deserialize)]
pub enum Module {
    Net,
    Chain,
    Miner,
    Pool,
    Trace,
    IntegrationTest,
    Wallet,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    pub listen_address: String,
    pub threads: Option<usize>,
    pub modules: Vec<Module>,
    pub max_request_body_size: usize,
    #[serde(default)]
    pub path: PathBuf,
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

    pub(crate) fn wallet_enable(&self) -> bool {
        self.modules.contains(&Module::Wallet)
    }
}
