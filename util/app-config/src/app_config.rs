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
use ckb_miner::BlockAssemblerConfig;
use ckb_miner::MinerConfig;
use ckb_network::NetworkConfig;
use ckb_resource::{Resource, ResourceLocator};
use ckb_rpc::Config as RpcConfig;
use ckb_shared::tx_pool::TxPoolConfig;
use ckb_sync::Config as SyncConfig;
use logger::Config as LogConfig;

use super::sentry_config::SentryConfig;
use super::{cli, ExitCode};

pub struct AppConfig {
    resource: Resource,
    content: AppConfigContent,
}

pub enum AppConfigContent {
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

    pub block_assembler: BlockAssemblerConfig,
    #[serde(skip)]
    pub db: DBConfig,
    pub network: NetworkConfig,
    pub rpc: RpcConfig,
    pub sync: SyncConfig,
    pub tx_pool: TxPoolConfig,
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
    pub spec: PathBuf,
}

impl AppConfig {
    pub fn is_bundled(&self) -> bool {
        self.resource.is_bundled()
    }

    pub fn load_for_subcommand(
        locator: &ResourceLocator,
        subcommand_name: &str,
    ) -> Result<AppConfig, ExitCode> {
        match subcommand_name {
            cli::CMD_MINER => {
                let resource = locator.miner();
                let config: MinerAppConfig = toml::from_slice(&resource.get()?)?;

                Ok(AppConfig {
                    resource,
                    content: AppConfigContent::with_miner(
                        config.derive_options(locator.root_dir())?,
                    ),
                })
            }
            _ => {
                let resource = locator.ckb();
                let config: CKBAppConfig = toml::from_slice(&resource.get()?)?;
                Ok(AppConfig {
                    resource,
                    content: AppConfigContent::with_ckb(
                        config.derive_options(locator.root_dir(), subcommand_name)?,
                    ),
                })
            }
        }
    }

    pub fn logger(&self) -> &LogConfig {
        match &self.content {
            AppConfigContent::CKB(config) => &config.logger,
            AppConfigContent::Miner(config) => &config.logger,
        }
    }

    pub fn sentry(&self) -> &SentryConfig {
        match &self.content {
            AppConfigContent::CKB(config) => &config.sentry,
            AppConfigContent::Miner(config) => &config.sentry,
        }
    }

    pub fn chain_spec(&self, locator: &ResourceLocator) -> Result<ChainSpec, ExitCode> {
        let spec_path = PathBuf::from(match &self.content {
            AppConfigContent::CKB(config) => &config.chain.spec,
            AppConfigContent::Miner(config) => &config.chain.spec,
        });
        ChainSpec::resolve_relative_to(locator, spec_path, &self.resource).map_err(|err| {
            eprintln!("{:?}", err);
            ExitCode::Config
        })
    }

    pub fn into_ckb(self) -> Result<Box<CKBAppConfig>, ExitCode> {
        match self.content {
            AppConfigContent::CKB(config) => Ok(config),
            _ => {
                eprintln!("unmatched config file");
                Err(ExitCode::Failure)
            }
        }
    }

    pub fn into_miner(self) -> Result<Box<MinerAppConfig>, ExitCode> {
        match self.content {
            AppConfigContent::Miner(config) => Ok(config),
            _ => {
                eprintln!("unmatched config file");
                Err(ExitCode::Failure)
            }
        }
    }
}

impl AppConfigContent {
    fn with_ckb(config: CKBAppConfig) -> AppConfigContent {
        AppConfigContent::CKB(Box::new(config))
    }
    fn with_miner(config: MinerAppConfig) -> AppConfigContent {
        AppConfigContent::Miner(Box::new(config))
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
        self.network.path = mkdir(self.data_dir.join("network"))?;

        Ok(self)
    }
}

impl MinerAppConfig {
    fn derive_options(mut self, root_dir: &Path) -> Result<Self, ExitCode> {
        self.data_dir = canonicalize_data_dir(self.data_dir, root_dir)?;
        if self.logger.log_to_file {
            self.logger.file = Some(touch(mkdir(self.data_dir.join("logs"))?.join("miner.log"))?);
        }

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
    dir.canonicalize().map_err(Into::into)
}

fn touch(path: PathBuf) -> Result<PathBuf, ExitCode> {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_resource::TemplateContext;

    fn mkdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("app_config_test")
            .tempdir()
            .unwrap()
    }

    #[test]
    fn test_ckb_toml() {
        let dir = mkdir();
        let locator = ResourceLocator::with_root_dir(dir.path().to_path_buf()).unwrap();
        let app_config = AppConfig::load_for_subcommand(&locator, cli::CMD_RUN)
            .unwrap_or_else(|err| panic!(err));
        let ckb_config = app_config.into_ckb().unwrap_or_else(|err| panic!(err));
        assert_eq!(ckb_config.chain.spec, PathBuf::from("specs/dev.toml"));
        assert_eq!(
            ckb_config.logger.file,
            Some(locator.root_dir().join("data/logs/run.log"))
        );
        assert_eq!(ckb_config.db.path, locator.root_dir().join("data/db"));
        assert_eq!(
            ckb_config.network.path,
            locator.root_dir().join("data/network")
        );
    }

    #[test]
    fn test_miner_toml() {
        let dir = mkdir();
        let locator = ResourceLocator::with_root_dir(dir.path().to_path_buf()).unwrap();
        let app_config = AppConfig::load_for_subcommand(&locator, cli::CMD_MINER)
            .unwrap_or_else(|err| panic!(err));
        let miner_config = app_config.into_miner().unwrap_or_else(|err| panic!(err));
        assert_eq!(miner_config.chain.spec, PathBuf::from("specs/dev.toml"));
        assert_eq!(
            miner_config.logger.file,
            Some(locator.root_dir().join("data/logs/miner.log"))
        );
    }

    #[test]
    fn test_export_dev_config_files() {
        let dir = mkdir();
        let locator = ResourceLocator::with_root_dir(dir.path().to_path_buf()).unwrap();
        let context = TemplateContext {
            spec: "dev",
            rpc_port: "7000",
            p2p_port: "8000",
            log_to_file: true,
            log_to_stdout: true,
        };
        {
            locator.export_ckb(&context).expect("export config files");
            let app_config = AppConfig::load_for_subcommand(&locator, cli::CMD_RUN)
                .unwrap_or_else(|err| panic!(err));
            let ckb_config = app_config.into_ckb().unwrap_or_else(|err| panic!(err));
            assert_eq!(ckb_config.logger.filter, Some("info".to_string()));
            assert_eq!(ckb_config.chain.spec, PathBuf::from("specs/dev.toml"));
            assert_eq!(
                ckb_config.network.listen_addresses,
                vec!["/ip4/0.0.0.0/tcp/8000".parse().unwrap()]
            );
            assert_eq!(ckb_config.network.connect_outbound_interval_secs, 15);
            assert_eq!(ckb_config.rpc.listen_address, "0.0.0.0:7000");
        }
        {
            locator.export_miner(&context).expect("export config files");
            let app_config = AppConfig::load_for_subcommand(&locator, cli::CMD_MINER)
                .unwrap_or_else(|err| panic!(err));
            let miner_config = app_config.into_miner().unwrap_or_else(|err| panic!(err));
            assert_eq!(miner_config.logger.filter, Some("info".to_string()));
            assert_eq!(miner_config.chain.spec, PathBuf::from("specs/dev.toml"));
            assert_eq!(miner_config.miner.rpc_url, "http://127.0.0.1:7000/");
        }
    }

    #[test]
    fn test_log_to_stdout_only() {
        let dir = mkdir();
        let locator = ResourceLocator::with_root_dir(dir.path().to_path_buf()).unwrap();
        let context = TemplateContext {
            spec: "dev",
            rpc_port: "7000",
            p2p_port: "8000",
            log_to_file: false,
            log_to_stdout: true,
        };
        {
            locator.export_ckb(&context).expect("export config files");
            let app_config = AppConfig::load_for_subcommand(&locator, cli::CMD_RUN)
                .unwrap_or_else(|err| panic!(err));
            let ckb_config = app_config.into_ckb().unwrap_or_else(|err| panic!(err));
            assert_eq!(ckb_config.logger.file, None);
            assert_eq!(ckb_config.logger.log_to_file, false);
            assert_eq!(ckb_config.logger.log_to_stdout, true);
        }
        {
            locator.export_miner(&context).expect("export config files");
            let app_config = AppConfig::load_for_subcommand(&locator, cli::CMD_MINER)
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
        let locator = ResourceLocator::with_root_dir(dir.path().to_path_buf()).unwrap();
        let context = TemplateContext {
            spec: "testnet",
            rpc_port: "7000",
            p2p_port: "8000",
            log_to_file: true,
            log_to_stdout: true,
        };
        locator.export_ckb(&context).expect("export config files");
        {
            let app_config = AppConfig::load_for_subcommand(&locator, cli::CMD_RUN)
                .unwrap_or_else(|err| panic!(err));
            let ckb_config = app_config.into_ckb().unwrap_or_else(|err| panic!(err));
            assert_eq!(ckb_config.logger.filter, Some("info".to_string()));
            assert_eq!(ckb_config.chain.spec, PathBuf::from("specs/testnet.toml"));
            assert_eq!(
                ckb_config.network.listen_addresses,
                vec!["/ip4/0.0.0.0/tcp/8000".parse().unwrap()]
            );
            assert_eq!(ckb_config.network.connect_outbound_interval_secs, 15);
            assert_eq!(ckb_config.rpc.listen_address, "0.0.0.0:7000");
        }
        {
            locator.export_miner(&context).expect("export config files");
            let app_config = AppConfig::load_for_subcommand(&locator, cli::CMD_MINER)
                .unwrap_or_else(|err| panic!(err));
            let miner_config = app_config.into_miner().unwrap_or_else(|err| panic!(err));
            assert_eq!(miner_config.logger.filter, Some("info".to_string()));
            assert_eq!(miner_config.chain.spec, PathBuf::from("specs/testnet.toml"));
            assert_eq!(miner_config.miner.rpc_url, "http://127.0.0.1:7000/");
        }
    }

    #[test]
    fn test_export_integration_config_files() {
        let dir = mkdir();
        let locator = ResourceLocator::with_root_dir(dir.path().to_path_buf()).unwrap();
        let context = TemplateContext {
            spec: "integration",
            rpc_port: "7000",
            p2p_port: "8000",
            log_to_file: true,
            log_to_stdout: true,
        };
        locator.export_ckb(&context).expect("export config files");
        {
            let app_config = AppConfig::load_for_subcommand(&locator, cli::CMD_RUN)
                .unwrap_or_else(|err| panic!(err));
            let ckb_config = app_config.into_ckb().unwrap_or_else(|err| panic!(err));
            assert_eq!(
                ckb_config.chain.spec,
                PathBuf::from("specs/integration.toml")
            );
            assert_eq!(
                ckb_config.network.listen_addresses,
                vec!["/ip4/0.0.0.0/tcp/8000".parse().unwrap()]
            );
            assert_eq!(ckb_config.network.connect_outbound_interval_secs, 1);
            assert_eq!(ckb_config.rpc.listen_address, "0.0.0.0:7000");
        }
        {
            locator.export_miner(&context).expect("export config files");
            let app_config = AppConfig::load_for_subcommand(&locator, cli::CMD_MINER)
                .unwrap_or_else(|err| panic!(err));
            let miner_config = app_config.into_miner().unwrap_or_else(|err| panic!(err));
            assert_eq!(
                miner_config.chain.spec,
                PathBuf::from("specs/integration.toml")
            );
            assert_eq!(miner_config.miner.rpc_url, "http://127.0.0.1:7000/");
        }
    }
}
