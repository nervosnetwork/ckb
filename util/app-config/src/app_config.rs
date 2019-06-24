//! # CKB AppConfig
//!
//! Because the limitation of toml library,
//! we must put nested config struct in the tail to make it serializable,
//! details https://docs.rs/toml/0.5.0/toml/ser/index.html

use std::fs;
use std::path::{Path, PathBuf};

use serde_derive::{Deserialize, Serialize};

use ckb_chain_spec::ChainSpec;
use ckb_db::DBConfig;
use ckb_logger::Config as LogConfig;
use ckb_miner::BlockAssemblerConfig;
use ckb_miner::MinerConfig;
use ckb_network::NetworkConfig;
use ckb_network_alert::config::Config as AlertConfig;
use ckb_resource::Resource;
use ckb_rpc::Config as RpcConfig;
use ckb_script::ScriptConfig;
use ckb_shared::tx_pool::TxPoolConfig;
use ckb_store::StoreConfig;
use ckb_sync::Config as SyncConfig;

use super::sentry_config::SentryConfig;
use super::ExitCode;

pub enum AppConfig {
    CKB(Box<CKBAppConfig>),
    Miner(Box<MinerAppConfig>),
}

// change the order of fields will break integration test, see module doc.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CKBAppConfig {
    pub data_dir: PathBuf,
    pub logger: LogConfig,
    pub sentry: SentryConfig,
    pub chain: ChainConfig,

    pub block_assembler: Option<BlockAssemblerConfig>,
    #[serde(default)]
    pub db: DBConfig,
    #[serde(default)]
    pub indexer_db: DBConfig,
    pub network: NetworkConfig,
    pub rpc: RpcConfig,
    pub sync: SyncConfig,
    pub tx_pool: TxPoolConfig,
    pub script: ScriptConfig,
    pub store: StoreConfig,
    pub alert: Option<AlertConfig>,
}

// change the order of fields will break integration test, see module doc.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MinerAppConfig {
    pub data_dir: PathBuf,
    pub chain: ChainConfig,
    pub logger: LogConfig,
    pub sentry: SentryConfig,

    pub miner: MinerConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChainConfig {
    pub spec: Resource,
}

const CMD_MINER: &str = "miner";

impl AppConfig {
    pub fn load_for_subcommand<P: AsRef<Path>>(
        root_dir: P,
        subcommand_name: &str,
    ) -> Result<AppConfig, ExitCode> {
        match subcommand_name {
            CMD_MINER => {
                let resource = ensure_ckb_dir(Resource::miner_config(root_dir.as_ref()))?;
                let config: MinerAppConfig = toml::from_slice(&resource.get()?)?;

                Ok(AppConfig::with_miner(
                    config.derive_options(root_dir.as_ref())?,
                ))
            }
            _ => {
                let resource = ensure_ckb_dir(Resource::ckb_config(root_dir.as_ref()))?;
                let config: CKBAppConfig = toml::from_slice(&resource.get()?)?;
                Ok(AppConfig::with_ckb(
                    config.derive_options(root_dir.as_ref(), subcommand_name)?,
                ))
            }
        }
    }

    pub fn logger(&self) -> &LogConfig {
        match self {
            AppConfig::CKB(config) => &config.logger,
            AppConfig::Miner(config) => &config.logger,
        }
    }

    pub fn sentry(&self) -> &SentryConfig {
        match self {
            AppConfig::CKB(config) => &config.sentry,
            AppConfig::Miner(config) => &config.sentry,
        }
    }

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

    pub fn into_ckb(self) -> Result<Box<CKBAppConfig>, ExitCode> {
        match self {
            AppConfig::CKB(config) => Ok(config),
            _ => {
                eprintln!("unmatched config file");
                Err(ExitCode::Failure)
            }
        }
    }

    pub fn into_miner(self) -> Result<Box<MinerAppConfig>, ExitCode> {
        match self {
            AppConfig::Miner(config) => Ok(config),
            _ => {
                eprintln!("unmatched config file");
                Err(ExitCode::Failure)
            }
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
    fn derive_options(mut self, root_dir: &Path, subcommand_name: &str) -> Result<Self, ExitCode> {
        self.data_dir = canonicalize_data_dir(self.data_dir, root_dir)?;
        if self.logger.log_to_file {
            self.logger.file = Some(touch(
                mkdir(self.data_dir.join("logs"))?.join(subcommand_name.to_string() + ".log"),
            )?);
        }
        self.db.path = mkdir(self.data_dir.join("db"))?;
        self.indexer_db.path = mkdir(self.data_dir.join("indexer_db"))?;
        self.network.path = mkdir(self.data_dir.join("network"))?;
        self.chain.spec.absolutize(root_dir);

        Ok(self)
    }
}

impl MinerAppConfig {
    fn derive_options(mut self, root_dir: &Path) -> Result<Self, ExitCode> {
        self.data_dir = canonicalize_data_dir(self.data_dir, root_dir)?;
        if self.logger.log_to_file {
            self.logger.file = Some(touch(mkdir(self.data_dir.join("logs"))?.join("miner.log"))?);
        }
        self.chain.spec.absolutize(root_dir);

        Ok(self)
    }
}

fn canonicalize_data_dir(data_dir: PathBuf, root_dir: &Path) -> Result<PathBuf, ExitCode> {
    let path = if data_dir.is_absolute() {
        data_dir
    } else {
        root_dir.join(data_dir)
    };

    mkdir(path)
}

fn mkdir(dir: PathBuf) -> Result<PathBuf, ExitCode> {
    fs::create_dir_all(&dir)?;
    // TODO: Use https://github.com/danreeves/path-clean to clean the
    // path if needed. std::fs::canonicalize will bring windows compatibility
    // problems
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

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_resource::TemplateContext;

    const CMD_RUN: &str = "run";

    fn mkdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("app_config_test")
            .tempdir()
            .unwrap()
    }

    #[test]
    fn test_bundled_config_files() {
        let resource = Resource::bundled_ckb_config();
        toml::from_slice::<CKBAppConfig>(&resource.get().expect("read bundled file"))
            .expect("deserialize config");

        let resource = Resource::bundled_miner_config();
        toml::from_slice::<MinerAppConfig>(&resource.get().expect("read bundled file"))
            .expect("deserialize config");
    }

    #[test]
    fn test_export_dev_config_files() {
        let dir = mkdir();
        let context = TemplateContext {
            spec: "dev",
            rpc_port: "7000",
            p2p_port: "8000",
            log_to_file: true,
            log_to_stdout: true,
            runner: "Rust",
            block_assembler: "",
        };
        {
            Resource::bundled_ckb_config()
                .export(&context, dir.path())
                .expect("export config files");
            let app_config = AppConfig::load_for_subcommand(dir.path(), CMD_RUN)
                .unwrap_or_else(|err| panic!(err));
            let ckb_config = app_config.into_ckb().unwrap_or_else(|err| panic!(err));
            assert_eq!(ckb_config.logger.filter, Some("info".to_string()));
            assert_eq!(
                ckb_config.chain.spec,
                Resource::file_system(dir.path().join("specs").join("dev.toml"))
            );
            assert_eq!(
                ckb_config.network.listen_addresses,
                vec!["/ip4/0.0.0.0/tcp/8000".parse().unwrap()]
            );
            assert_eq!(ckb_config.network.connect_outbound_interval_secs, 15);
            assert_eq!(ckb_config.rpc.listen_address, "127.0.0.1:7000");
        }
        {
            Resource::bundled_miner_config()
                .export(&context, dir.path())
                .expect("export config files");
            let app_config = AppConfig::load_for_subcommand(dir.path(), CMD_MINER)
                .unwrap_or_else(|err| panic!(err));
            let miner_config = app_config.into_miner().unwrap_or_else(|err| panic!(err));
            assert_eq!(miner_config.logger.filter, Some("info".to_string()));
            assert_eq!(
                miner_config.chain.spec,
                Resource::file_system(dir.path().join("specs").join("dev.toml"))
            );
            assert_eq!(miner_config.miner.client.rpc_url, "http://127.0.0.1:7000/");
        }
    }

    #[test]
    fn test_log_to_stdout_only() {
        let dir = mkdir();
        let context = TemplateContext {
            spec: "dev",
            rpc_port: "7000",
            p2p_port: "8000",
            log_to_file: false,
            log_to_stdout: true,
            runner: "Rust",
            block_assembler: "",
        };
        {
            Resource::bundled_ckb_config()
                .export(&context, dir.path())
                .expect("export config files");
            let app_config = AppConfig::load_for_subcommand(dir.path(), CMD_RUN)
                .unwrap_or_else(|err| panic!(err));
            let ckb_config = app_config.into_ckb().unwrap_or_else(|err| panic!(err));
            assert_eq!(ckb_config.logger.file, None);
            assert_eq!(ckb_config.logger.log_to_file, false);
            assert_eq!(ckb_config.logger.log_to_stdout, true);
        }
        {
            Resource::bundled_miner_config()
                .export(&context, dir.path())
                .expect("export config files");
            let app_config = AppConfig::load_for_subcommand(dir.path(), CMD_MINER)
                .unwrap_or_else(|err| panic!(err));
            let miner_config = app_config.into_miner().unwrap_or_else(|err| panic!(err));
            assert_eq!(miner_config.logger.file, None);
            assert_eq!(miner_config.logger.log_to_file, false);
            assert_eq!(miner_config.logger.log_to_stdout, true);
        }
    }

    #[test]
    fn test_export_testnet_config_files() {
        let dir = mkdir();
        let context = TemplateContext {
            spec: "testnet",
            rpc_port: "7000",
            p2p_port: "8000",
            log_to_file: true,
            log_to_stdout: true,
            runner: "Rust",
            block_assembler: "",
        };
        {
            Resource::bundled_ckb_config()
                .export(&context, dir.path())
                .expect("export config files");
            let app_config = AppConfig::load_for_subcommand(dir.path(), CMD_RUN)
                .unwrap_or_else(|err| panic!(err));
            let ckb_config = app_config.into_ckb().unwrap_or_else(|err| panic!(err));
            assert_eq!(ckb_config.logger.filter, Some("info".to_string()));
            assert_eq!(
                ckb_config.chain.spec,
                Resource::bundled("specs/testnet.toml".to_string())
            );
            assert_eq!(
                ckb_config.network.listen_addresses,
                vec!["/ip4/0.0.0.0/tcp/8000".parse().unwrap()]
            );
            assert_eq!(ckb_config.network.connect_outbound_interval_secs, 15);
            assert_eq!(ckb_config.rpc.listen_address, "127.0.0.1:7000");
        }
        {
            Resource::bundled_miner_config()
                .export(&context, dir.path())
                .expect("export config files");
            let app_config = AppConfig::load_for_subcommand(dir.path(), CMD_MINER)
                .unwrap_or_else(|err| panic!(err));
            let miner_config = app_config.into_miner().unwrap_or_else(|err| panic!(err));
            assert_eq!(miner_config.logger.filter, Some("info".to_string()));
            assert_eq!(
                miner_config.chain.spec,
                Resource::bundled("specs/testnet.toml".to_string())
            );
            assert_eq!(miner_config.miner.client.rpc_url, "http://127.0.0.1:7000/");
        }
    }

    #[test]
    fn test_export_integration_config_files() {
        let dir = mkdir();
        let context = TemplateContext {
            spec: "integration",
            rpc_port: "7000",
            p2p_port: "8000",
            log_to_file: true,
            log_to_stdout: true,
            runner: "Rust",
            block_assembler: "",
        };
        {
            Resource::bundled_ckb_config()
                .export(&context, dir.path())
                .expect("export config files");
            let app_config = AppConfig::load_for_subcommand(dir.path(), CMD_RUN)
                .unwrap_or_else(|err| panic!(err));
            let ckb_config = app_config.into_ckb().unwrap_or_else(|err| panic!(err));
            assert_eq!(
                ckb_config.chain.spec,
                Resource::file_system(dir.path().join("specs").join("integration.toml"))
            );
            assert_eq!(
                ckb_config.network.listen_addresses,
                vec!["/ip4/0.0.0.0/tcp/8000".parse().unwrap()]
            );
            assert_eq!(ckb_config.rpc.listen_address, "127.0.0.1:7000");
        }
        {
            Resource::bundled_miner_config()
                .export(&context, dir.path())
                .expect("export config files");
            let app_config = AppConfig::load_for_subcommand(dir.path(), CMD_MINER)
                .unwrap_or_else(|err| panic!(err));
            let miner_config = app_config.into_miner().unwrap_or_else(|err| panic!(err));
            assert_eq!(
                miner_config.chain.spec,
                Resource::file_system(dir.path().join("specs").join("integration.toml"))
            );
            assert_eq!(miner_config.miner.client.rpc_url, "http://127.0.0.1:7000/");
        }
    }

    #[cfg(all(unix, target_pointer_width = "64"))]
    #[test]
    fn test_export_dev_config_files_assembly() {
        let dir = mkdir();
        let context = TemplateContext {
            spec: "dev",
            rpc_port: "7000",
            p2p_port: "8000",
            log_to_file: true,
            log_to_stdout: true,
            runner: "Assembly",
            block_assembler: "",
        };
        {
            Resource::bundled_ckb_config()
                .export(&context, dir.path())
                .expect("export config files");
            let app_config = AppConfig::load_for_subcommand(dir.path(), CMD_RUN)
                .unwrap_or_else(|err| panic!(err));
            let ckb_config = app_config.into_ckb().unwrap_or_else(|err| panic!(err));
            assert_eq!(ckb_config.logger.filter, Some("info".to_string()));
            assert_eq!(
                ckb_config.chain.spec,
                Resource::file_system(dir.path().join("specs").join("dev.toml"))
            );
            assert_eq!(
                ckb_config.network.listen_addresses,
                vec!["/ip4/0.0.0.0/tcp/8000".parse().unwrap()]
            );
            assert_eq!(ckb_config.network.connect_outbound_interval_secs, 15);
            assert_eq!(ckb_config.rpc.listen_address, "127.0.0.1:7000");
        }
        {
            Resource::bundled_miner_config()
                .export(&context, dir.path())
                .expect("export config files");
            let app_config = AppConfig::load_for_subcommand(dir.path(), CMD_MINER)
                .unwrap_or_else(|err| panic!(err));
            let miner_config = app_config.into_miner().unwrap_or_else(|err| panic!(err));
            assert_eq!(miner_config.logger.filter, Some("info".to_string()));
            assert_eq!(
                miner_config.chain.spec,
                Resource::file_system(dir.path().join("specs").join("dev.toml"))
            );
            assert_eq!(miner_config.miner.client.rpc_url, "http://127.0.0.1:7000/");
        }
    }
}
