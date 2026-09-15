use crate::configs::default_max_tx_verify_workers;
use ckb_chain_spec::consensus::TWO_IN_TWO_OUT_CYCLES;
use ckb_jsonrpc_types::FeeRateDef;
use ckb_types::core::{Cycle, FeeRate};
use serde::Deserialize;
use std::cmp;
#[cfg(feature = "test")]
use std::num::NonZeroU32;
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
    #[cfg(feature = "test")]
    #[serde(default = "default_max_tx_verify_time_ms")]
    max_tx_verify_time_ms: NonZeroU32,
    max_ancestors_count: usize,
    #[serde(default)]
    persisted_data: PathBuf,
    #[serde(default)]
    recent_reject: PathBuf,
    #[serde(default = "default_expiry_hours")]
    expiry_hours: u8,
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

fn default_min_rbf_rate() -> FeeRate {
    DEFAULT_MIN_RBF_RATE
}

#[cfg(feature = "test")]
const fn default_max_tx_verify_time_ms() -> NonZeroU32 {
    NonZeroU32::new(ckb_chain_spec::consensus::MIN_BLOCK_INTERVAL as u32 * 1_000).unwrap()
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
            max_tx_verify_workers: default_max_tx_verify_workers(),
            keep_rejected_tx_hashes_days: default_keep_rejected_tx_hashes_days(),
            keep_rejected_tx_hashes_count: default_keep_rejected_tx_hashes_count(),
            min_fee_rate: DEFAULT_MIN_FEE_RATE,
            min_rbf_rate: DEFAULT_MIN_RBF_RATE,
            max_tx_verify_cycles: DEFAULT_MAX_TX_VERIFY_CYCLES,
            #[cfg(feature = "test")]
            max_tx_verify_time_ms: default_max_tx_verify_time_ms(),
            max_ancestors_count: DEFAULT_MAX_ANCESTORS_COUNT,
            persisted_data: Default::default(),
            recent_reject: Default::default(),
            expiry_hours: DEFAULT_EXPIRY_HOURS,
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
            max_tx_verify_workers,
            keep_rejected_tx_hashes_days,
            keep_rejected_tx_hashes_count,
            min_fee_rate,
            min_rbf_rate,
            max_tx_verify_cycles,
            #[cfg(feature = "test")]
            max_tx_verify_time_ms,
            max_ancestors_count,
            persisted_data,
            recent_reject,
            expiry_hours,
        } = input;

        Self {
            max_tx_pool_size,
            min_fee_rate,
            min_rbf_rate,
            max_tx_verify_cycles,
            #[cfg(feature = "test")]
            max_tx_verify_time_ms,
            max_tx_verify_workers,
            max_ancestors_count: cmp::max(DEFAULT_MAX_ANCESTORS_COUNT, max_ancestors_count),
            keep_rejected_tx_hashes_days,
            keep_rejected_tx_hashes_count,
            persisted_data,
            recent_reject,
            expiry_hours,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(feature = "test"))]
    fn network_time_cap_is_not_a_user_configuration() {
        let config = crate::TxPoolConfig::default();
        assert_eq!(
            config.max_tx_verify_time(),
            std::time::Duration::from_secs(ckb_chain_spec::consensus::MIN_BLOCK_INTERVAL)
        );
        let value = toml::Value::try_from(config).unwrap();
        let _: TxPoolConfig = value.clone().try_into().unwrap();
        for field in [
            "max_tx_verify_time_ms",
            "min_tx_verify_time_ms",
            "cycles_per_ms",
        ] {
            assert!(!value.as_table().unwrap().contains_key(field));
            let mut configured = value.clone();
            configured
                .as_table_mut()
                .unwrap()
                .insert(field.into(), toml::Value::Integer(1));
            assert!(configured.try_into::<TxPoolConfig>().is_err(), "{field}");
        }
    }

    #[test]
    #[cfg(feature = "test")]
    fn test_build_can_override_the_network_time_cap() {
        let mut value = toml::Value::try_from(crate::TxPoolConfig::default()).unwrap();
        let fields = value.as_table_mut().unwrap();
        fields.remove("max_tx_verify_time_ms");
        assert!(!fields.contains_key("min_tx_verify_time_ms"));
        assert!(!fields.contains_key("cycles_per_ms"));
        let old: TxPoolConfig = value.clone().try_into().unwrap();
        assert_eq!(
            crate::TxPoolConfig::from(old).max_tx_verify_time_ms.get(),
            8_000
        );

        value
            .as_table_mut()
            .unwrap()
            .insert("max_tx_verify_time_ms".into(), toml::Value::Integer(12_000));
        let configured: TxPoolConfig = value.clone().try_into().unwrap();
        assert_eq!(
            crate::TxPoolConfig::from(configured).max_tx_verify_time(),
            std::time::Duration::from_millis(12_000)
        );
        value["max_tx_verify_time_ms"] = toml::Value::Integer(0);
        assert!(value.try_into::<TxPoolConfig>().is_err());
    }
}
