use ckb_chain_spec::consensus::TWO_IN_TWO_OUT_CYCLES;
use ckb_jsonrpc_types::FeeRateDef;
use ckb_types::core::{Cycle, FeeRate};
use serde::Deserialize;
use std::cmp;
use std::path::PathBuf;

// default min fee rate, 1000 shannons per kilobyte
const DEFAULT_MIN_FEE_RATE: FeeRate = FeeRate::from_u64(1000);
// default min rbf rate, 1500 shannons per kilobyte
const DEFAULT_MIN_RBF_RATE: FeeRate = FeeRate::from_u64(1500);
// default max tx verify cycles
const DEFAULT_MAX_TX_VERIFY_CYCLES: Cycle = TWO_IN_TWO_OUT_CYCLES * 20;
// default max ancestors count
const DEFAULT_MAX_ANCESTORS_COUNT: usize = 125;
// Default expiration time for pool transactions in hours
const DEFAULT_EXPIRY_HOURS: u8 = 12;
// Default max_tx_pool_size 180mb
const DEFAULT_MAX_TX_POOL_SIZE: usize = 180_000_000;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct TxPoolConfig {
    #[serde(default = "default_max_tx_pool_size")]
    max_tx_pool_size: usize,
    max_mem_size: Option<usize>,
    max_cycles: Option<Cycle>,
    pub(crate) max_verify_cache_size: Option<usize>,
    pub(crate) max_conflict_cache_size: Option<usize>,
    pub(crate) max_committed_txs_hash_cache_size: Option<usize>,
    #[serde(default = "default_keep_rejected_tx_hashes_days")]
    keep_rejected_tx_hashes_days: u8,
    #[serde(default = "default_keep_rejected_tx_hashes_count")]
    keep_rejected_tx_hashes_count: u64,
    #[serde(with = "FeeRateDef")]
    min_fee_rate: FeeRate,
    #[serde(with = "FeeRateDef")]
    min_rbf_rate: FeeRate,
    max_tx_verify_cycles: Cycle,
    max_ancestors_count: usize,
    #[serde(default)]
    persisted_data: PathBuf,
    #[serde(default)]
    recent_reject: PathBuf,
    #[serde(default = "default_expiry_hours")]
    expiry_hours: u8,
    #[serde(default)]
    enable_rbf: bool,
}

fn default_keep_rejected_tx_hashes_days() -> u8 {
    7
}

fn default_keep_rejected_tx_hashes_count() -> u64 {
    10_000_000
}

fn default_expiry_hours() -> u8 {
    DEFAULT_EXPIRY_HOURS
}

fn default_max_tx_pool_size() -> usize {
    DEFAULT_MAX_TX_POOL_SIZE
}

impl Default for crate::TxPoolConfig {
    fn default() -> Self {
        TxPoolConfig::default().into()
    }
}

impl Default for TxPoolConfig {
    fn default() -> Self {
        Self {
            max_mem_size: None,
            max_tx_pool_size: DEFAULT_MAX_TX_POOL_SIZE,
            max_cycles: None,
            max_verify_cache_size: None,
            max_conflict_cache_size: None,
            max_committed_txs_hash_cache_size: None,
            keep_rejected_tx_hashes_days: default_keep_rejected_tx_hashes_days(),
            keep_rejected_tx_hashes_count: default_keep_rejected_tx_hashes_count(),
            min_fee_rate: DEFAULT_MIN_FEE_RATE,
            min_rbf_rate: DEFAULT_MIN_RBF_RATE,
            max_tx_verify_cycles: DEFAULT_MAX_TX_VERIFY_CYCLES,
            max_ancestors_count: DEFAULT_MAX_ANCESTORS_COUNT,
            persisted_data: Default::default(),
            recent_reject: Default::default(),
            expiry_hours: DEFAULT_EXPIRY_HOURS,
            enable_rbf: false,
        }
    }
}

impl From<TxPoolConfig> for crate::TxPoolConfig {
    fn from(input: TxPoolConfig) -> Self {
        let TxPoolConfig {
            max_mem_size: _,
            max_tx_pool_size,
            max_cycles: _,
            max_verify_cache_size: _,
            max_conflict_cache_size: _,
            max_committed_txs_hash_cache_size: _,
            keep_rejected_tx_hashes_days,
            keep_rejected_tx_hashes_count,
            min_fee_rate,
            min_rbf_rate,
            max_tx_verify_cycles,
            max_ancestors_count,
            persisted_data,
            recent_reject,
            expiry_hours,
            enable_rbf,
        } = input;

        Self {
            max_tx_pool_size,
            min_fee_rate,
            min_rbf_rate,
            max_tx_verify_cycles,
            max_ancestors_count: cmp::max(DEFAULT_MAX_ANCESTORS_COUNT, max_ancestors_count),
            keep_rejected_tx_hashes_days,
            keep_rejected_tx_hashes_count,
            persisted_data,
            recent_reject,
            expiry_hours,
            enable_rbf,
        }
    }
}
