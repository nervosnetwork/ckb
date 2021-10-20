use ckb_chain_spec::consensus::TWO_IN_TWO_OUT_CYCLES;
use ckb_jsonrpc_types::FeeRateDef;
use ckb_types::core::{Cycle, FeeRate};
use serde::Deserialize;
use std::path::PathBuf;

// default min fee rate, 1000 shannons per kilobyte
const DEFAULT_MIN_FEE_RATE: FeeRate = FeeRate::from_u64(1000);
// default max tx verify cycles
const DEFAULT_MAX_TX_VERIFY_CYCLES: Cycle = TWO_IN_TWO_OUT_CYCLES * 20;
// default max ancestors count
const DEFAULT_MAX_ANCESTORS_COUNT: usize = 25;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TxPoolConfig {
    max_mem_size: usize,
    max_cycles: Cycle,
    pub(crate) max_verify_cache_size: Option<usize>,
    pub(crate) max_conflict_cache_size: Option<usize>,
    pub(crate) max_committed_txs_hash_cache_size: Option<usize>,
    #[serde(with = "FeeRateDef")]
    min_fee_rate: FeeRate,
    max_tx_verify_cycles: Cycle,
    max_ancestors_count: usize,
    #[serde(default)]
    persisted_data: PathBuf,
}

impl Default for crate::TxPoolConfig {
    fn default() -> Self {
        TxPoolConfig::default().into()
    }
}

impl Default for TxPoolConfig {
    fn default() -> Self {
        Self {
            max_mem_size: 20_000_000, // 20mb
            max_cycles: 200_000_000_000,
            max_verify_cache_size: None,
            max_conflict_cache_size: None,
            max_committed_txs_hash_cache_size: None,
            min_fee_rate: DEFAULT_MIN_FEE_RATE,
            max_tx_verify_cycles: DEFAULT_MAX_TX_VERIFY_CYCLES,
            max_ancestors_count: DEFAULT_MAX_ANCESTORS_COUNT,
            persisted_data: Default::default(),
        }
    }
}

impl From<TxPoolConfig> for crate::TxPoolConfig {
    fn from(input: TxPoolConfig) -> Self {
        let TxPoolConfig {
            max_mem_size,
            max_cycles,
            max_verify_cache_size: _,
            max_conflict_cache_size: _,
            max_committed_txs_hash_cache_size: _,
            min_fee_rate,
            max_tx_verify_cycles,
            max_ancestors_count,
            persisted_data,
        } = input;
        Self {
            max_mem_size,
            max_cycles,
            min_fee_rate,
            max_tx_verify_cycles,
            max_ancestors_count,
            persisted_data,
        }
    }
}
