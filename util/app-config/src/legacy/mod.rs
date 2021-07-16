//! Legacy CKB AppConfig and Miner AppConfig

use serde::Deserialize;
use std::path::PathBuf;

mod store;
mod tx_pool;

pub(crate) struct DeprecatedField {
    pub(crate) path: &'static str,
    // The first version which doesn't include the field.
    pub(crate) since: &'static str,
}

//
// The core legacy structs.
//

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CKBAppConfig {
    data_dir: PathBuf,
    #[serde(default)]
    ancient: PathBuf,
    tmp_dir: Option<PathBuf>,
    logger: crate::LogConfig,

    #[cfg(feature = "with_sentry")]
    #[serde(default)]
    sentry: crate::SentryConfig,
    #[cfg(not(feature = "with_sentry"))]
    #[serde(default)]
    sentry: serde_json::Value,

    #[serde(default)]
    metrics: crate::MetricsConfig,
    #[serde(default)]
    memory_tracker: crate::MemoryTrackerConfig,
    chain: crate::ChainConfig,
    block_assembler: Option<crate::BlockAssemblerConfig>,
    #[serde(default)]
    db: crate::DBConfig,

    #[serde(default)]
    indexer: Option<serde_json::Value>,

    network: crate::NetworkConfig,
    rpc: crate::RpcConfig,

    tx_pool: tx_pool::TxPoolConfig,

    #[serde(default)]
    store: store::StoreConfig,

    alert_signature: Option<crate::NetworkAlertConfig>,
    #[serde(default)]
    notify: crate::NotifyConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MinerAppConfig {
    data_dir: PathBuf,
    chain: crate::ChainConfig,
    logger: crate::LogConfig,

    #[cfg(feature = "with_sentry")]
    sentry: crate::SentryConfig,
    #[cfg(not(feature = "with_sentry"))]
    #[serde(default)]
    sentry: serde_json::Value,

    #[serde(default)]
    metrics: crate::MetricsConfig,
    #[serde(default)]
    memory_tracker: crate::MemoryTrackerConfig,
    miner: crate::MinerConfig,
}

//
// The conversion which convert legacy structs to latest structs.
//

impl From<CKBAppConfig> for crate::CKBAppConfig {
    fn from(input: CKBAppConfig) -> Self {
        let CKBAppConfig {
            data_dir,
            ancient,
            tmp_dir,
            logger,
            sentry,
            metrics,
            memory_tracker,
            chain,
            block_assembler,
            db,
            indexer: _,
            network,
            rpc,
            tx_pool,
            store,
            alert_signature,
            notify,
        } = input;
        #[cfg(not(feature = "with_sentry"))]
        let _ = sentry;
        Self {
            data_dir,
            ancient,
            tmp_dir,
            logger,
            #[cfg(feature = "with_sentry")]
            sentry,
            metrics,
            memory_tracker,
            chain,
            block_assembler,
            db,
            network,
            rpc,
            tx_pool: tx_pool.into(),
            store: store.into(),
            alert_signature,
            notify,
        }
    }
}

impl From<MinerAppConfig> for crate::MinerAppConfig {
    fn from(input: MinerAppConfig) -> Self {
        let MinerAppConfig {
            data_dir,
            chain,
            logger,
            sentry,
            metrics,
            memory_tracker,
            miner,
        } = input;
        #[cfg(not(feature = "with_sentry"))]
        let _ = sentry;
        Self {
            data_dir,
            chain,
            logger,
            #[cfg(feature = "with_sentry")]
            sentry,
            metrics,
            memory_tracker,
            miner,
        }
    }
}

//
// The core functions.
//

impl DeprecatedField {
    pub(crate) fn new(path: &'static str, since: &'static str) -> Self {
        Self { path, since }
    }
}

macro_rules! deprecate {
    ($self:ident, $fields:ident, $field:ident $(. $path:ident)*, $since:literal) => {
        if $self. $field $(. $path)* .is_some() {
            let path = concat!(stringify!($field), $(".", stringify!($path),)*);
            $fields.push(DeprecatedField::new(path, $since));
        }
    };
}

impl CKBAppConfig {
    pub(crate) fn deprecated_fields(&self) -> Vec<DeprecatedField> {
        let mut v = Vec::new();
        deprecate!(self, v, indexer, "0.40.0");
        deprecate!(self, v, store.cellbase_cache_size, "0.100.0");
        deprecate!(self, v, tx_pool.max_verify_cache_size, "0.100.0");
        deprecate!(self, v, tx_pool.max_conflict_cache_size, "0.100.0");
        deprecate!(
            self,
            v,
            tx_pool.max_committed_txs_hash_cache_size,
            "0.100.0"
        );
        v
    }
}

impl MinerAppConfig {
    pub(crate) fn deprecated_fields(&self) -> Vec<DeprecatedField> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{CKBAppConfig, DeprecatedField, MinerAppConfig};
    use ckb_resource::{Resource, TemplateContext, AVAILABLE_SPECS};

    fn mkdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("ckb_app_config_test")
            .tempdir()
            .unwrap()
    }

    #[test]
    fn macro_deprecate_works_well() {
        struct Config {
            first: Option<usize>,
            second: AConfig,
        }
        struct AConfig {
            a_f1: Option<usize>,
            a_f2: BConfig,
        }
        struct BConfig {
            b_f1: Option<usize>,
        }

        let c = Config {
            first: Some(0),
            second: AConfig {
                a_f1: Some(1),
                a_f2: BConfig { b_f1: Some(2) },
            },
        };
        let deprecated_fields = {
            let mut v = Vec::new();
            deprecate!(c, v, first, "0.1.0");
            deprecate!(c, v, second.a_f1, "0.2.0");
            deprecate!(c, v, second.a_f2.b_f1, "0.3.0");
            v
        };
        assert_eq!(deprecated_fields.len(), 3);
        assert_eq!(deprecated_fields[0].path, "first");
        assert_eq!(deprecated_fields[1].path, "second.a_f1");
        assert_eq!(deprecated_fields[2].path, "second.a_f2.b_f1");
    }

    #[test]
    fn no_deprecated_fields_in_bundled_ckb_app_config() {
        let root_dir = mkdir();
        for name in AVAILABLE_SPECS {
            let context = TemplateContext::new(
                name,
                vec![
                    ("rpc_port", "7000"),
                    ("p2p_port", "8000"),
                    ("log_to_file", "true"),
                    ("log_to_stdout", "true"),
                    ("block_assembler", ""),
                    ("spec_source", "bundled"),
                ],
            );
            Resource::bundled_ckb_config()
                .export(&context, root_dir.path())
                .expect("export ckb.toml");
            let resource = Resource::ckb_config(root_dir.path());
            let legacy_config: CKBAppConfig =
                toml::from_slice(&resource.get().expect("resource get slice"))
                    .expect("toml load slice");
            assert!(legacy_config.deprecated_fields().is_empty());
        }
    }

    #[test]
    fn no_deprecated_fields_in_bundled_miner_app_config() {
        let root_dir = mkdir();
        for name in AVAILABLE_SPECS {
            let context = TemplateContext::new(
                name,
                vec![
                    ("log_to_file", "true"),
                    ("log_to_stdout", "true"),
                    ("spec_source", "bundled"),
                ],
            );
            Resource::bundled_miner_config()
                .export(&context, root_dir.path())
                .expect("export ckb-miner.toml");
            let resource = Resource::miner_config(root_dir.path());
            let legacy_config: MinerAppConfig =
                toml::from_slice(&resource.get().expect("resource get slice"))
                    .expect("toml load slice");
            assert!(legacy_config.deprecated_fields().is_empty());
        }
    }
}
