//! # CKB AppConfig
//!
//! Because the limitation of toml library,
//! we must put nested config struct in the tail to make it serializable,
//! details https://docs.rs/toml/0.5.0/toml/ser/index.html

use path_clean::PathClean;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use ckb_chain_spec::ChainSpec;
pub use ckb_logger_config::Config as LogConfig;
pub use ckb_metrics_config::Config as MetricsConfig;
use ckb_resource::Resource;

use super::configs::*;
#[cfg(feature = "with_sentry")]
use super::sentry_config::SentryConfig;
use super::{cli, legacy, ExitCode};

/// The parsed config file.
///
/// CKB process reads `ckb.toml` or `ckb-miner.toml`, depending what subcommand to be executed.
pub enum AppConfig {
    /// The parsed `ckb.toml.`
    CKB(Box<CKBAppConfig>),
    /// The parsed `ckb-miner.toml.`
    Miner(Box<MinerAppConfig>),
}

/// The main config file for the most subcommands. Usually it is the `ckb.toml` in the CKB root
/// directory.
///
/// **Attention:** Changing the order of fields will break integration test, see module doc.
#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CKBAppConfig {
    /// The binary name.
    #[serde(skip)]
    pub bin_name: String,
    /// The root directory.
    #[serde(skip)]
    pub root_dir: PathBuf,
    /// The data directory.
    pub data_dir: PathBuf,
    /// freezer files path
    #[serde(default)]
    pub ancient: PathBuf,
    /// The directory to store temporary files.
    pub tmp_dir: Option<PathBuf>,
    /// Logger config options.
    pub logger: LogConfig,
    /// Sentry config options.
    #[cfg(feature = "with_sentry")]
    #[serde(default)]
    pub sentry: SentryConfig,
    /// Metrics options.
    ///
    /// Developers can collect metrics for performance tuning and troubleshooting.
    #[serde(default)]
    pub metrics: MetricsConfig,
    /// Memory tracker options.
    ///
    /// Developers can enable memory tracker to analyze the process memory usage.
    #[serde(default)]
    pub memory_tracker: MemoryTrackerConfig,
    /// Chain config options.
    pub chain: ChainConfig,

    /// Block assembler options.
    pub block_assembler: Option<BlockAssemblerConfig>,
    /// Database config options.
    #[serde(default)]
    pub db: DBConfig,
    /// Network config options.
    pub network: NetworkConfig,
    /// RPC config options.
    pub rpc: RpcConfig,
    /// Tx pool config options.
    pub tx_pool: TxPoolConfig,
    /// Store config options.
    #[serde(default)]
    pub store: StoreConfig,
    /// P2P alert config options.
    pub alert_signature: Option<NetworkAlertConfig>,
    /// Notify config options.
    #[serde(default)]
    pub notify: NotifyConfig,
    /// Indexer config options.
    #[serde(default)]
    pub indexer: IndexerConfig,
}

/// The miner config file for `ckb miner`. Usually it is the `ckb-miner.toml` in the CKB root
/// directory.
///
/// **Attention:** Changing the order of fields will break integration test, see module doc.
#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MinerAppConfig {
    /// The binary name.
    #[serde(skip)]
    pub bin_name: String,
    /// The root directory.
    #[serde(skip)]
    pub root_dir: PathBuf,
    /// The data directory.
    pub data_dir: PathBuf,
    /// Chain config options.
    pub chain: ChainConfig,
    /// Logger config options.
    pub logger: LogConfig,
    /// Sentry config options.
    #[cfg(feature = "with_sentry")]
    pub sentry: SentryConfig,
    /// Metrics options.
    ///
    /// Developers can collect metrics for performance tuning and troubleshooting.
    #[serde(default)]
    pub metrics: MetricsConfig,
    /// Memory tracker options.
    ///
    /// Developers can enable memory tracker to analyze the process memory usage.
    #[serde(default)]
    pub memory_tracker: MemoryTrackerConfig,

    /// The miner config options.
    pub miner: MinerConfig,
}

/// The chain config options.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainConfig {
    /// Specifies the chain spec.
    pub spec: Resource,
}

impl AppConfig {
    /// Reads the config file for the subcommand.
    ///
    /// This will reads the `ckb-miner.toml` in the CKB directory for `ckb miner`, and `ckb.toml`
    /// for all other subcommands.
    pub fn load_for_subcommand<P: AsRef<Path>>(
        root_dir: P,
        subcommand_name: &str,
    ) -> Result<AppConfig, ExitCode> {
        match subcommand_name {
            cli::CMD_MINER => {
                let resource = ensure_ckb_dir(Resource::miner_config(root_dir.as_ref()))?;
                let config = MinerAppConfig::load_from_slice(&resource.get()?)?;

                Ok(AppConfig::with_miner(
                    config.derive_options(root_dir.as_ref())?,
                ))
            }
            _ => {
                let resource = ensure_ckb_dir(Resource::ckb_config(root_dir.as_ref()))?;
                let config = CKBAppConfig::load_from_slice(&resource.get()?)?;

                Ok(AppConfig::with_ckb(
                    config.derive_options(root_dir.as_ref(), subcommand_name)?,
                ))
            }
        }
    }

    /// Gets logger options.
    pub fn logger(&self) -> &LogConfig {
        match self {
            AppConfig::CKB(config) => &config.logger,
            AppConfig::Miner(config) => &config.logger,
        }
    }

    /// Gets sentry options.
    #[cfg(feature = "with_sentry")]
    pub fn sentry(&self) -> &SentryConfig {
        match self {
            AppConfig::CKB(config) => &config.sentry,
            AppConfig::Miner(config) => &config.sentry,
        }
    }

    /// Gets metrics options.
    pub fn metrics(&self) -> &MetricsConfig {
        match self {
            AppConfig::CKB(config) => &config.metrics,
            AppConfig::Miner(config) => &config.metrics,
        }
    }

    /// Gets memory tracker options.
    pub fn memory_tracker(&self) -> &MemoryTrackerConfig {
        match self {
            AppConfig::CKB(config) => &config.memory_tracker,
            AppConfig::Miner(config) => &config.memory_tracker,
        }
    }

    /// Gets chain spec.
    pub fn chain_spec(&self) -> Result<ChainSpec, ExitCode> {
        let spec_resource = match self {
            AppConfig::CKB(config) => &config.chain.spec,
            AppConfig::Miner(config) => &config.chain.spec,
        };
        ChainSpec::load_from(spec_resource).map_err(|err| {
            eprintln!("{}", err);
            ExitCode::Config
        })
    }

    /// Unpacks the parsed ckb.toml config file.
    ///
    /// Panics when this is a parsed ckb-miner.toml.
    pub fn into_ckb(self) -> Result<Box<CKBAppConfig>, ExitCode> {
        match self {
            AppConfig::CKB(config) => Ok(config),
            _ => {
                eprintln!("unmatched config file");
                Err(ExitCode::Failure)
            }
        }
    }

    /// Unpacks the parsed ckb-miner.toml config file.
    ///
    /// Panics when this is a parsed ckb.toml.
    pub fn into_miner(self) -> Result<Box<MinerAppConfig>, ExitCode> {
        match self {
            AppConfig::Miner(config) => Ok(config),
            _ => {
                eprintln!("unmatched config file");
                Err(ExitCode::Failure)
            }
        }
    }

    /// Set the binary name with full path.
    pub fn set_bin_name(&mut self, bin_name: String) {
        match self {
            AppConfig::CKB(config) => config.bin_name = bin_name,
            AppConfig::Miner(config) => config.bin_name = bin_name,
        }
    }
}

impl AppConfig {
    fn with_ckb(config: CKBAppConfig) -> AppConfig {
        AppConfig::CKB(Box::new(config))
    }
    fn with_miner(config: MinerAppConfig) -> AppConfig {
        AppConfig::Miner(Box::new(config))
    }
}

impl CKBAppConfig {
    /// Load a new instance from a file
    pub fn load_from_slice(slice: &[u8]) -> Result<Self, ExitCode> {
        let legacy_config: legacy::CKBAppConfig = toml::from_slice(slice)?;
        for field in legacy_config.deprecated_fields() {
            eprintln!(
                "WARN: the option \"{}\" in configuration files is deprecated since v{}.",
                field.path, field.since
            );
        }
        Ok(legacy_config.into())
    }

    fn derive_options(mut self, root_dir: &Path, subcommand_name: &str) -> Result<Self, ExitCode> {
        self.root_dir = root_dir.to_path_buf();

        self.data_dir = canonicalize_data_dir(self.data_dir, root_dir);

        self.db.adjust(root_dir, &self.data_dir, "db");
        self.ancient = mkdir(path_specified_or_else(&self.ancient, || {
            self.data_dir.join("ancient")
        }))?;

        self.network.path = self.data_dir.join("network");
        if self.tmp_dir.is_none() {
            self.tmp_dir = Some(self.data_dir.join("tmp"));
        }
        self.logger.log_dir = self.data_dir.join("logs");
        self.logger.file = Path::new(&(subcommand_name.to_string() + ".log")).to_path_buf();

        let tx_pool_path = mkdir(self.data_dir.join("tx_pool"))?;
        self.tx_pool.adjust(root_dir, tx_pool_path);

        let indexer_path = mkdir(self.data_dir.join("indexer"))?;
        self.indexer.adjust(root_dir, indexer_path);

        if subcommand_name == cli::CMD_RESET_DATA {
            return Ok(self);
        }

        self.data_dir = mkdir(self.data_dir)?;
        self.db.path = mkdir(self.db.path)?;
        self.network.path = mkdir(self.network.path)?;
        if let Some(tmp_dir) = self.tmp_dir {
            self.tmp_dir = Some(mkdir(tmp_dir)?);
        }
        if self.logger.log_to_file {
            mkdir(self.logger.log_dir.clone())?;
            touch(self.logger.log_dir.join(&self.logger.file))?;
        }
        self.chain.spec.absolutize(root_dir);

        Ok(self)
    }
}

impl MinerAppConfig {
    /// Load a new instance from a file.
    pub fn load_from_slice(slice: &[u8]) -> Result<Self, ExitCode> {
        let legacy_config: legacy::MinerAppConfig = toml::from_slice(slice)?;
        for field in legacy_config.deprecated_fields() {
            eprintln!(
                "WARN: the option \"{}\" in configuration files is deprecated since v{}.",
                field.path, field.since
            );
        }
        Ok(legacy_config.into())
    }

    fn derive_options(mut self, root_dir: &Path) -> Result<Self, ExitCode> {
        self.root_dir = root_dir.to_path_buf();

        self.data_dir = mkdir(canonicalize_data_dir(self.data_dir, root_dir))?;
        self.logger.log_dir = self.data_dir.join("logs");
        self.logger.file = Path::new("miner.log").to_path_buf();
        if self.logger.log_to_file {
            mkdir(self.logger.log_dir.clone())?;
            touch(self.logger.log_dir.join(&self.logger.file))?;
        }
        self.chain.spec.absolutize(root_dir);

        Ok(self)
    }
}

fn canonicalize_data_dir(data_dir: PathBuf, root_dir: &Path) -> PathBuf {
    if data_dir.is_absolute() {
        data_dir
    } else {
        root_dir.join(data_dir)
    }
}

fn mkdir(dir: PathBuf) -> Result<PathBuf, ExitCode> {
    fs::create_dir_all(&dir.clean())?;
    // std::fs::canonicalize will bring windows compatibility problems
    Ok(dir)
}

fn touch(path: PathBuf) -> Result<PathBuf, ExitCode> {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;

    Ok(path)
}

fn ensure_ckb_dir(r: Resource) -> Result<Resource, ExitCode> {
    if r.exists() {
        Ok(r)
    } else {
        eprintln!("Not a CKB directory, initialize one with `ckb init`.");
        Err(ExitCode::Config)
    }
}

fn path_specified_or_else<P: AsRef<Path>, F: FnOnce() -> PathBuf>(
    path: P,
    default_path: F,
) -> PathBuf {
    let path_ref = path.as_ref();
    if path_ref.to_str().is_none() || path_ref.to_str() == Some("") {
        default_path()
    } else {
        path_ref.to_path_buf()
    }
}
