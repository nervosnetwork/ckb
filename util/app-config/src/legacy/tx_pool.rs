use crate::configs::{VerifyOrdering, default_max_tx_verify_workers};
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
const DEFAULT_MAX_ANCESTORS_COUNT: usize = 1_000;
// Default expiration time for pool transactions in hours
const DEFAULT_EXPIRY_HOURS: u8 = 12;
// Default max_tx_pool_size 180mb
const DEFAULT_MAX_TX_POOL_SIZE: usize = 180_000_000;
// Default conservative accepted-pool residency budget. This is deliberately
// larger than the serialized transaction limit so ordinary workloads retain
// their historical pool capacity while dep-group expansion remains bounded.
const DEFAULT_MAX_TX_POOL_RESIDENT_SIZE: usize = 1_000_000_000;
// Default conservative resident-byte budget for the complete pre-pool
// pipeline. This preserves the previous 64 MB pre-check + 64 MB ordered
// resolve + 256 MB verify allocation under the unified coordinator.
const DEFAULT_MAX_TX_PIPELINE_RESIDENT_SIZE: usize = 384_000_000;
const LEGACY_PRE_VERIFY_PIPELINE_BYTES: usize = 128_000_000;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub(crate) struct TxPoolConfig {
    #[serde(default = "default_max_tx_pool_size")]
    max_tx_pool_size: usize,
    #[serde(default = "default_max_tx_pool_resident_size")]
    max_tx_pool_resident_size: usize,
    max_mem_size: Option<usize>,
    max_cycles: Option<Cycle>,
    pub(crate) max_verify_cache_size: Option<usize>,
    pub(crate) max_conflict_cache_size: Option<usize>,
    pub(crate) max_committed_txs_hash_cache_size: Option<usize>,
    #[serde(default = "default_max_tx_verify_workers")]
    max_tx_verify_workers: usize,
    #[serde(default = "default_keep_rejected_tx_hashes_days")]
    keep_rejected_tx_hashes_days: u8,
    #[serde(default = "default_keep_rejected_tx_hashes_count")]
    keep_rejected_tx_hashes_count: u64,
    #[serde(with = "FeeRateDef")]
    min_fee_rate: FeeRate,
    #[serde(with = "FeeRateDef", default = "default_min_rbf_rate")]
    min_rbf_rate: FeeRate,
    max_tx_verify_cycles: Cycle,
    max_ancestors_count: usize,
    #[serde(default)]
    persisted_data: PathBuf,
    #[serde(default)]
    recent_reject: PathBuf,
    #[serde(default = "default_expiry_hours")]
    expiry_hours: u8,
    #[serde(default = "default_verify_ordering")]
    verify_ordering: VerifyOrdering,
    /// New unified resident budget. `Option` preserves whether the user
    /// explicitly configured it so the removed verify-only field can be
    /// translated without silently shrinking the old aggregate capacity.
    #[serde(default)]
    max_tx_pipeline_resident_size: Option<usize>,
    /// Backward-compatible input only. The old architecture also owned two
    /// fixed 64 MB pre-verify queues, added during conversion below.
    #[serde(default)]
    max_verify_queue_tx_size: Option<usize>,
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

fn default_verify_ordering() -> VerifyOrdering {
    VerifyOrdering::ArrivalTime
}

fn default_max_tx_pool_size() -> usize {
    DEFAULT_MAX_TX_POOL_SIZE
}

fn default_max_tx_pool_resident_size() -> usize {
    DEFAULT_MAX_TX_POOL_RESIDENT_SIZE
}

fn default_min_rbf_rate() -> FeeRate {
    DEFAULT_MIN_RBF_RATE
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
            max_tx_pool_resident_size: DEFAULT_MAX_TX_POOL_RESIDENT_SIZE,
            max_cycles: None,
            max_verify_cache_size: None,
            max_conflict_cache_size: None,
            max_committed_txs_hash_cache_size: None,
            max_tx_verify_workers: default_max_tx_verify_workers(),
            keep_rejected_tx_hashes_days: default_keep_rejected_tx_hashes_days(),
            keep_rejected_tx_hashes_count: default_keep_rejected_tx_hashes_count(),
            min_fee_rate: DEFAULT_MIN_FEE_RATE,
            min_rbf_rate: DEFAULT_MIN_RBF_RATE,
            max_tx_verify_cycles: DEFAULT_MAX_TX_VERIFY_CYCLES,
            max_ancestors_count: DEFAULT_MAX_ANCESTORS_COUNT,
            persisted_data: Default::default(),
            recent_reject: Default::default(),
            expiry_hours: DEFAULT_EXPIRY_HOURS,
            verify_ordering: VerifyOrdering::ArrivalTime,
            max_tx_pipeline_resident_size: Some(DEFAULT_MAX_TX_PIPELINE_RESIDENT_SIZE),
            max_verify_queue_tx_size: None,
        }
    }
}

impl From<TxPoolConfig> for crate::TxPoolConfig {
    fn from(input: TxPoolConfig) -> Self {
        let TxPoolConfig {
            max_mem_size: _,
            max_tx_pool_size,
            max_tx_pool_resident_size,
            max_cycles: _,
            max_verify_cache_size: _,
            max_conflict_cache_size: _,
            max_committed_txs_hash_cache_size: _,
            max_tx_verify_workers,
            keep_rejected_tx_hashes_days,
            keep_rejected_tx_hashes_count,
            min_fee_rate,
            min_rbf_rate,
            max_tx_verify_cycles,
            max_ancestors_count,
            persisted_data,
            recent_reject,
            expiry_hours,
            verify_ordering,
            max_tx_pipeline_resident_size,
            max_verify_queue_tx_size,
        } = input;

        let max_tx_pipeline_resident_size = max_tx_pipeline_resident_size.unwrap_or_else(|| {
            max_verify_queue_tx_size
                .map(|verify_bytes| verify_bytes.saturating_add(LEGACY_PRE_VERIFY_PIPELINE_BYTES))
                .unwrap_or(DEFAULT_MAX_TX_PIPELINE_RESIDENT_SIZE)
        });

        Self {
            max_tx_pool_size,
            max_tx_pool_resident_size,
            min_fee_rate,
            min_rbf_rate,
            max_tx_verify_cycles,
            max_tx_verify_workers,
            max_ancestors_count: cmp::max(DEFAULT_MAX_ANCESTORS_COUNT, max_ancestors_count),
            keep_rejected_tx_hashes_days,
            keep_rejected_tx_hashes_count,
            persisted_data,
            recent_reject,
            expiry_hours,
            verify_ordering,
            max_tx_pipeline_resident_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED_FIELDS: &str = r#"
min_fee_rate = 1000
max_tx_verify_cycles = 70_000_000
max_ancestors_count = 25
"#;

    fn parse(extra: &str) -> crate::TxPoolConfig {
        toml::from_str::<TxPoolConfig>(&format!("{REQUIRED_FIELDS}\n{extra}"))
            .expect("parse legacy tx-pool config")
            .into()
    }

    #[test]
    fn legacy_verify_budget_preserves_the_old_aggregate_pipeline_capacity() {
        let config = parse("max_verify_queue_tx_size = 256_000_000");
        assert_eq!(config.max_tx_pipeline_resident_size, 384_000_000);
    }

    #[test]
    fn explicit_unified_pipeline_budget_takes_precedence_over_legacy_input() {
        let config = parse(
            "max_verify_queue_tx_size = 256_000_000\n\
             max_tx_pipeline_resident_size = 512_000_000",
        );
        assert_eq!(config.max_tx_pipeline_resident_size, 512_000_000);
    }

    #[test]
    fn omitted_pipeline_budget_keeps_the_unified_default() {
        let config = parse("");
        assert_eq!(
            config.max_tx_pipeline_resident_size,
            DEFAULT_MAX_TX_PIPELINE_RESIDENT_SIZE
        );
    }
}
