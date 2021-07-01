//! Legacy CKB AppConfig (Edition 2019)

use ckb_jsonrpc_types as rpc;
use ckb_types::H256;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CKBAppConfig {
    data_dir: PathBuf,
    #[serde(default)]
    ancient: PathBuf,
    tmp_dir: Option<PathBuf>,
    logger: crate::LogConfig,
    #[cfg(feature = "with_sentry")]
    #[serde(default)]
    sentry: crate::SentryConfig,
    #[serde(default)]
    metrics: crate::MetricsConfig,
    #[serde(default)]
    memory_tracker: crate::MemoryTrackerConfig,
    chain: crate::ChainConfig,
    block_assembler: Option<BlockAssemblerConfig>,
    #[serde(default)]
    db: crate::DBConfig,
    network: crate::NetworkConfig,
    rpc: crate::RpcConfig,
    tx_pool: crate::TxPoolConfig,
    #[serde(default)]
    store: crate::StoreConfig,
    alert_signature: Option<crate::NetworkAlertConfig>,
    #[serde(default)]
    notify: crate::NotifyConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct MinerAppConfig {
    data_dir: PathBuf,
    chain: crate::ChainConfig,
    logger: crate::LogConfig,
    #[cfg(feature = "with_sentry")]
    sentry: crate::SentryConfig,
    #[serde(default)]
    metrics: crate::MetricsConfig,
    #[serde(default)]
    memory_tracker: crate::MemoryTrackerConfig,
    miner: crate::MinerConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BlockAssemblerConfig {
    code_hash: H256,
    hash_type: rpc::ScriptHashTypeKind,
    args: rpc::JsonBytes,
    message: rpc::JsonBytes,
}

impl From<BlockAssemblerConfig> for crate::BlockAssemblerConfig {
    fn from(input: BlockAssemblerConfig) -> Self {
        let BlockAssemblerConfig {
            code_hash,
            hash_type: hash_type_kind,
            args,
            message,
        } = input;
        let hash_type = match hash_type_kind {
            rpc::ScriptHashTypeKind::Data => rpc::ScriptHashType::Data { vm_version: 0 },
            rpc::ScriptHashTypeKind::Type => rpc::ScriptHashType::Type,
        };
        Self {
            code_hash,
            hash_type,
            args,
            message,
        }
    }
}

impl From<CKBAppConfig> for crate::CKBAppConfig {
    fn from(input: CKBAppConfig) -> Self {
        let CKBAppConfig {
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
            tx_pool,
            store,
            alert_signature,
            notify,
        } = input;
        Self {
            edition: rpc::ChainEdition::V2021,
            data_dir,
            ancient,
            tmp_dir,
            logger,
            #[cfg(feature = "with_sentry")]
            sentry,
            metrics,
            memory_tracker,
            chain,
            block_assembler: block_assembler.map(Into::into),
            db,
            network,
            rpc,
            tx_pool,
            store,
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
            #[cfg(feature = "with_sentry")]
            sentry,
            metrics,
            memory_tracker,
            miner,
        } = input;
        Self {
            edition: rpc::ChainEdition::V2021,
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
