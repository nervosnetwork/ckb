use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChainConfig {
    #[serde(default = "default_tx_block_stat_enable")]
    tx_block_stat_enable: bool,
    #[serde(default = "default_spec")]
    spec: Resource,
}

const fn default_spec() -> Resource {
    Resource::default()
}

const fn default_tx_block_stat_enable() -> bool {
    false
}

impl Default for crate::ChainConfig {
    fn default() -> Self {
        ChainConfig::default().into()
    }
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            spec: default_spec(),
            tx_block_stat_enable: default_tx_block_stat_enable(),
        }
    }
}

impl From<ChainConfig> for crate::ChainConfig {
    fn from(input: ChainConfig) -> Self {
        let ChainConfig {
            spec,
            tx_block_stat_enable,
        } = input;
        Self {
            spec,
            tx_block_stat_enable,
        }
    }
}
