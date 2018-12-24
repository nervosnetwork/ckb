use serde_derive::Deserialize;

const NET: &str = "Net";
const CHAIN: &str = "Chain";
const MINER: &str = "Miner";
const POOL: &str = "Pool";

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    pub listen_address: String,
    pub threads: Option<usize>,
    pub modules: Vec<String>,
}

impl Config {
    pub(crate) fn net_enable(&self) -> bool {
        self.modules.iter().any(|m| m == NET)
    }

    pub(crate) fn chain_enable(&self) -> bool {
        self.modules.iter().any(|m| m == CHAIN)
    }

    pub(crate) fn miner_enable(&self) -> bool {
        self.modules.iter().any(|m| m == MINER)
    }

    pub(crate) fn pool_enable(&self) -> bool {
        self.modules.iter().any(|m| m == POOL)
    }
}
