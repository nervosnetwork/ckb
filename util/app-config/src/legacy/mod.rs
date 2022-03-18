//! Legacy CKB AppConfig and Miner AppConfig

use crate::BIN_NAME;
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
            bin_name: BIN_NAME.to_owned(),
            root_dir: Default::default(),
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
            bin_name: BIN_NAME.to_owned(),
            root_dir: Default::default(),
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

#[doc(hidden)]
#[macro_export]
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
