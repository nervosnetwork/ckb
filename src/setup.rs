use ckb_chain_spec::ChainSpec;
use ckb_miner::Config as MinerConfig;
use ckb_network::Config as NetworkConfig;
use ckb_pool::txs_pool::PoolConfig;
use ckb_rpc::Config as RpcConfig;
use ckb_sync::Config as SyncConfig;
use clap::ArgMatches;
use config_tool::{Config as ConfigTool, File};
use dir::Directories;
use logger::Config as LogConfig;
use serde_derive::Deserialize;
use std::error::Error;
use std::path::{Path, PathBuf};

const DEFAULT_CONFIG_PATHS: &[&str] = &["ckb.json", "nodes/default.json"];

#[derive(Clone, Debug)]
pub struct Setup {
    pub configs: Configs,
    pub chain_spec: ChainSpec,
    pub dirs: Directories,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CKB {
    pub chain: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Configs {
    pub data_dir: PathBuf,
    pub ckb: CKB,
    pub logger: LogConfig,
    pub network: NetworkConfig,
    pub rpc: RpcConfig,
    pub miner: MinerConfig,
    pub sync: SyncConfig,
    pub pool: PoolConfig,
}

pub fn get_config_path(matches: &ArgMatches) -> PathBuf {
    to_absolute_path(
        matches
            .value_of("config")
            .map_or_else(find_default_config_path, |v| {
                require_path_exists(PathBuf::from(v))
            })
            .unwrap_or_else(|| {
                eprintln!("No config file found!");
                ::std::process::exit(1);
            }),
    )
}

impl Setup {
    pub(crate) fn with_configs(mut configs: Configs) -> Result<Self, Box<Error>> {
        let dirs = Directories::new(&configs.data_dir);

        if let Some(file) = configs.logger.file {
            let mut path = dirs.join("logs");
            path.push(file);
            configs.logger.file = Some(path.to_str().unwrap().to_string());
        }
        if configs.network.config_dir_path.is_none() {
            configs.network.config_dir_path =
                Some(dirs.join("network").to_string_lossy().to_string());
        }

        let chain_spec = ChainSpec::read_from_file(&configs.ckb.chain)?;

        Ok(Setup {
            configs,
            chain_spec,
            dirs,
        })
    }

    pub fn setup<T: AsRef<Path>>(config_path: T) -> Result<Self, Box<Error>> {
        let mut config_tool = ConfigTool::new();

        config_tool.merge(File::from(config_path.as_ref()))?;

        let mut configs: Configs = config_tool.try_into()?;
        configs.resolve_paths(config_path.as_ref().parent().unwrap());

        Self::with_configs(configs)
    }
}

impl Configs {
    fn resolve_paths(&mut self, base: &Path) {
        if self.data_dir.is_relative() {
            self.data_dir = base.join(&self.data_dir);
        }
        if self.ckb.chain.is_relative() {
            self.ckb.chain = base.join(&self.ckb.chain);
        }
    }
}

fn find_default_config_path() -> Option<PathBuf> {
    DEFAULT_CONFIG_PATHS
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

fn require_path_exists(path: PathBuf) -> Option<PathBuf> {
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

fn to_absolute_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        let mut absulute_path = ::std::env::current_dir().expect("get current_dir");
        absulute_path.push(path);
        absulute_path
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use config_tool::File as ConfigFile;
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;
    use tempfile;

    fn override_default_config_file<T: AsRef<Path>>(config_path: &T) -> Result<Setup, Box<Error>> {
        let mut config_tool = ConfigTool::new();
        let default_config_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("nodes_template/default.json");
        config_tool.merge(ConfigFile::from(default_config_path.as_path()))?;
        config_tool.merge(ConfigFile::from(config_path.as_ref()))?;

        let mut configs: Configs = config_tool.try_into()?;
        configs.resolve_paths(default_config_path.parent().unwrap());

        Setup::with_configs(configs)
    }

    fn write_file<P: AsRef<Path>>(file: P, content: &str) {
        let mut file = File::create(file).expect("test dir clean");
        file.write_all(content.as_bytes())
            .expect("write test content");;
    }

    fn test_chain_spec() -> &'static str {
        r#"
        {
            "name": "ckb_test_custom",
            "genesis": {
                "seal": {
                    "nonce": 233,
                    "proof": [2, 3, 3]
                },
                "version": 0,
                "parent_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "timestamp": 0,
                "txs_commit": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "txs_proposal": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "difficulty": "0x233",
                "cellbase_id": "0x0000000000000000000000000000000000000000000000000000000000000000",
                "uncles_hash": "0x0000000000000000000000000000000000000000000000000000000000000000"
            },
            "params": {
                "initial_block_reward": 233
            },
            "system_cells": [
                {"path": "verify"},
                {"path": "always_success"}
            ],
            "pow": {
                "Cuckoo": {
                    "edge_bits": 29,
                    "cycle_length": 42
                }
            }
        }
        "#
    }

    #[test]
    fn test_load_config() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_load_config")
            .tempdir()
            .unwrap();

        let test_conifg = r#"{
            "network": {
                "listen_addresses": ["/ip4/1.1.1.1/tcp/1"]
            }
        }"#;
        let config_path = tmp_dir.path().join("config.json");
        write_file(&config_path, test_conifg);
        let setup = override_default_config_file(&config_path);
        assert!(setup.is_ok());
        assert_eq!(
            setup.unwrap().configs.network.listen_addresses,
            vec!["/ip4/1.1.1.1/tcp/1".parse().unwrap()]
        );
    }

    #[test]
    fn test_custom_chain_spec_with_config() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_custom_chain_spec_with_config")
            .tempdir()
            .unwrap();

        let chain_spec_path = tmp_dir.path().join("ckb_test_custom.json");
        let test_conifg = format!(
            r#"
        {{
            "ckb": {{
                "chain": "{}"
            }}
        }}"#,
            chain_spec_path.to_str().unwrap()
        );

        let config_path = tmp_dir.path().join("config.json");
        write_file(&config_path, &test_conifg);
        write_file(&chain_spec_path, test_chain_spec());

        let setup = override_default_config_file(&config_path);
        assert!(setup.is_ok());
        assert_eq!(setup.unwrap().chain_spec.name, "ckb_test_custom");
    }
}
