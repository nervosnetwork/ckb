use crate::configs::{VerifyOrdering, default_max_tx_verify_workers};
use ckb_chain_spec::consensus::{MIN_BLOCK_INTERVAL, TWO_IN_TWO_OUT_CYCLES};
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
const DEFAULT_MIN_TX_VERIFY_TIME_MS: u32 = 250;
const DEFAULT_TX_VERIFY_CYCLES_PER_MS: u64 = 10_000;
// A tx-pool attempt may execute at most one consensus minimum block-interval
// quantum of cumulative active CKB-VM verification work. Queueing, suspension
// and non-script checks are excluded. This is node-local admission policy
// only; block verification remains governed exclusively by consensus cycles.
const DEFAULT_MAX_TX_VERIFY_TIME_MS: u32 = MIN_BLOCK_INTERVAL as u32 * 1_000;
const DEFAULT_MAX_TX_VERIFY_INITIAL_LOAD_BYTES: u64 = 256 * 1024 * 1024;
// default max ancestors count
const DEFAULT_MAX_ANCESTORS_COUNT: usize = 1_000;
// Legacy files normalize smaller configured values to this compatibility floor.
const LEGACY_MIN_ANCESTORS_COUNT: usize = 1_000;
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
    #[serde(default = "default_min_tx_verify_time_ms")]
    min_tx_verify_time_ms: u32,
    #[serde(default = "default_tx_verify_cycles_per_ms")]
    tx_verify_cycles_per_ms: u64,
    #[serde(default = "default_max_tx_verify_time_ms")]
    max_tx_verify_time_ms: u32,
    #[serde(default = "default_max_tx_verify_initial_load_bytes")]
    max_tx_verify_initial_load_bytes: u64,
    max_ancestors_count: usize,
    #[serde(default)]
    persisted_data: PathBuf,
    #[serde(default)]
    recent_reject: PathBuf,
    #[serde(default = "default_expiry_hours")]
    expiry_hours: u8,
    #[serde(default)]
    verify_ordering: VerifyOrdering,
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

fn default_min_tx_verify_time_ms() -> u32 {
    DEFAULT_MIN_TX_VERIFY_TIME_MS
}

fn default_tx_verify_cycles_per_ms() -> u64 {
    DEFAULT_TX_VERIFY_CYCLES_PER_MS
}

fn default_max_tx_verify_time_ms() -> u32 {
    DEFAULT_MAX_TX_VERIFY_TIME_MS
}

fn default_max_tx_verify_initial_load_bytes() -> u64 {
    DEFAULT_MAX_TX_VERIFY_INITIAL_LOAD_BYTES
}

impl Default for crate::TxPoolConfig {
    fn default() -> Self {
        Self {
            max_ancestors_count: DEFAULT_MAX_ANCESTORS_COUNT,
            ..TxPoolConfig::default().into()
        }
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
            min_tx_verify_time_ms: DEFAULT_MIN_TX_VERIFY_TIME_MS,
            tx_verify_cycles_per_ms: DEFAULT_TX_VERIFY_CYCLES_PER_MS,
            max_tx_verify_time_ms: DEFAULT_MAX_TX_VERIFY_TIME_MS,
            max_tx_verify_initial_load_bytes: DEFAULT_MAX_TX_VERIFY_INITIAL_LOAD_BYTES,
            max_ancestors_count: DEFAULT_MAX_ANCESTORS_COUNT,
            persisted_data: Default::default(),
            recent_reject: Default::default(),
            expiry_hours: DEFAULT_EXPIRY_HOURS,
            verify_ordering: VerifyOrdering::default(),
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
            min_tx_verify_time_ms,
            tx_verify_cycles_per_ms,
            max_tx_verify_time_ms,
            max_tx_verify_initial_load_bytes,
            max_ancestors_count,
            persisted_data,
            recent_reject,
            expiry_hours,
            verify_ordering,
        } = input;

        Self {
            max_tx_pool_size,
            min_fee_rate,
            min_rbf_rate,
            max_tx_verify_cycles,
            min_tx_verify_time_ms,
            tx_verify_cycles_per_ms,
            max_tx_verify_time_ms,
            max_tx_verify_initial_load_bytes,
            max_tx_verify_workers,
            max_ancestors_count: cmp::max(LEGACY_MIN_ANCESTORS_COUNT, max_ancestors_count),
            keep_rejected_tx_hashes_days,
            keep_rejected_tx_hashes_count,
            persisted_data,
            recent_reject,
            expiry_hours,
            verify_ordering,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The tx_pool section shipped before this PR (f75d609f9).
    const RELEASED_FIELDS: &str = r#"
max_tx_pool_size = 180_000_000
min_fee_rate = 1_000
min_rbf_rate = 1_500
max_tx_verify_cycles = 70_000_000
max_ancestors_count = 25
"#;

    fn parse(extra: &str) -> crate::TxPoolConfig {
        toml::from_str::<TxPoolConfig>(&format!("{RELEASED_FIELDS}\n{extra}"))
            .expect("parse tx-pool config")
            .into()
    }

    #[test]
    fn released_config_upgrades_with_default_verification_policy() {
        let config = parse("");
        assert_eq!(config.max_tx_pool_size, 180_000_000);
        assert_eq!(config.min_fee_rate, FeeRate::from_u64(1_000));
        assert_eq!(config.min_rbf_rate, FeeRate::from_u64(1_500));
        assert_eq!(config.max_tx_verify_cycles, 70_000_000);
        assert_eq!(config.max_ancestors_count, 1_000);
        assert_eq!(config.verify_ordering, VerifyOrdering::FeeRate);
        assert_eq!(config.min_tx_verify_time_ms, 250);
        assert_eq!(config.tx_verify_cycles_per_ms, 10_000);
        assert_eq!(
            u64::from(config.max_tx_verify_time_ms),
            MIN_BLOCK_INTERVAL * 1_000
        );
        assert_eq!(config.max_tx_verify_initial_load_bytes, 268_435_456);
        assert_eq!(
            config.max_tx_verify_workers,
            default_max_tx_verify_workers()
        );
        assert_eq!(config.expiry_hours, 12);
        assert_eq!(config.keep_rejected_tx_hashes_days, 7);
        assert_eq!(config.keep_rejected_tx_hashes_count, 10_000_000);
        assert!(config.persisted_data.as_os_str().is_empty());
        assert!(config.recent_reject.as_os_str().is_empty());
    }

    #[test]
    fn explicit_released_fields_keep_their_values() {
        let legacy: TxPoolConfig = toml::from_str(
            r#"
max_tx_pool_size = 271_000_000
min_fee_rate = 1_100
min_rbf_rate = 1_600
max_tx_verify_cycles = 80_000_000
max_tx_verify_workers = 3
max_ancestors_count = 1_234
keep_rejected_tx_hashes_days = 5
keep_rejected_tx_hashes_count = 2_000
persisted_data = "custom/persisted"
recent_reject = "custom/rejected"
expiry_hours = 9
"#,
        )
        .unwrap();
        let config: crate::TxPoolConfig = legacy.into();
        assert_eq!(config.max_tx_pool_size, 271_000_000);
        assert_eq!(config.min_fee_rate, FeeRate::from_u64(1_100));
        assert_eq!(config.min_rbf_rate, FeeRate::from_u64(1_600));
        assert_eq!(config.max_tx_verify_cycles, 80_000_000);
        assert_eq!(config.max_tx_verify_workers, 3);
        assert_eq!(config.max_ancestors_count, 1_234);
        assert_eq!(config.keep_rejected_tx_hashes_days, 5);
        assert_eq!(config.keep_rejected_tx_hashes_count, 2_000);
        assert_eq!(config.persisted_data, PathBuf::from("custom/persisted"));
        assert_eq!(config.recent_reject, PathBuf::from("custom/rejected"));
        assert_eq!(config.expiry_hours, 9);
    }

    #[test]
    fn obsolete_released_fields_are_accepted_without_changing_policy() {
        let config = parse(
            r#"
max_mem_size = 17
max_cycles = 23
max_verify_cache_size = 31
max_conflict_cache_size = 37
max_committed_txs_hash_cache_size = 41
"#,
        );
        assert_eq!(
            serde_json::to_value(config).unwrap(),
            serde_json::to_value(parse("")).unwrap(),
            "previously ignored fields must not acquire new resource semantics"
        );
    }

    #[test]
    fn ancestor_default_and_legacy_normalization_preserve_their_separate_policies() {
        assert_eq!(crate::TxPoolConfig::default().max_ancestors_count, 1_000);
        for (configured, expected) in [(0, 1_000), (25, 1_000), (1_000, 1_000), (1_001, 1_001)] {
            let text = RELEASED_FIELDS.replace(
                "max_ancestors_count = 25",
                &format!("max_ancestors_count = {configured}"),
            );
            let legacy: TxPoolConfig = toml::from_str(&text).unwrap();
            let config: crate::TxPoolConfig = legacy.into();
            assert_eq!(config.max_ancestors_count, expected);
        }
    }

    #[test]
    fn ordering_defaults_to_fee_rate_and_preserves_explicit_selection() {
        assert_eq!(
            crate::TxPoolConfig::default().verify_ordering,
            VerifyOrdering::FeeRate
        );
        assert_eq!(parse("").verify_ordering, VerifyOrdering::FeeRate);
        for (name, expected) in [
            ("fee_rate", VerifyOrdering::FeeRate),
            ("arrival_time", VerifyOrdering::ArrivalTime),
        ] {
            assert_eq!(
                parse(&format!("verify_ordering = \"{name}\"")).verify_ordering,
                expected
            );
        }
        assert!(
            toml::from_str::<TxPoolConfig>(&format!(
                "{RELEASED_FIELDS}\nverify_ordering = \"unknown\""
            ))
            .is_err()
        );
    }

    #[test]
    fn explicit_verification_policy_survives_conversion_exactly() {
        let config = parse(
            r#"
min_tx_verify_time_ms = 125
tx_verify_cycles_per_ms = 5_000
max_tx_verify_time_ms = 12_000
max_tx_verify_initial_load_bytes = 134_217_728
"#,
        );
        assert_eq!(config.min_tx_verify_time_ms, 125);
        assert_eq!(config.tx_verify_cycles_per_ms, 5_000);
        assert_eq!(config.max_tx_verify_time_ms, 12_000);
        assert_eq!(config.max_tx_verify_initial_load_bytes, 134_217_728);
    }

    #[test]
    fn unknown_and_unreleased_fields_are_rejected() {
        for field in [
            "max_tx_pool_szie",
            "max_tx_pool_resident_size",
            "max_tx_pipeline_resident_size",
            "max_verify_queue_tx_size",
        ] {
            let error = toml::from_str::<TxPoolConfig>(&format!("{RELEASED_FIELDS}\n{field} = 1"))
                .expect_err("only supported configuration keys are accepted");
            assert!(
                error.to_string().contains("unknown field"),
                "{field}: {error}"
            );
        }
    }
}
