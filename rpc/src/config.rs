use serde_derive::Deserialize;

#[derive(Clone, Debug, Copy, Eq, PartialEq, Deserialize)]
pub enum Module {
    Net,
    Chain,
    Miner,
    Pool,
    Trace,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    pub listen_address: String,
    pub threads: Option<usize>,
    pub modules: Vec<Module>,
    pub max_request_body_size: usize,
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
}
