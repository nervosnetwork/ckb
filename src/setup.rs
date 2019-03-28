use crate::helper::{require_path_exists, to_absolute_path};
use ckb_chain_spec::{ChainSpec, SpecPath};
use ckb_db::DBConfig;
use ckb_miner::BlockAssemblerConfig;
use ckb_network::NetworkConfig;
use ckb_rpc::Config as RpcConfig;
use ckb_shared::tx_pool::TxPoolConfig;
use ckb_sync::Config as SyncConfig;
use clap::ArgMatches;
use config_tool::{Config as ConfigTool, ConfigError, File};
use dir::Directories;
use logger::Config as LogConfig;
use serde_derive::Deserialize;
use std::error::Error;
use std::path::{Path, PathBuf};

const DEFAULT_CONFIG_PATHS: &[&str] = &["ckb.toml", "nodes/default.toml"];

#[derive(Clone, Debug)]
pub struct Setup {
    pub configs: Configs,
    pub chain_spec: ChainSpec,
    pub dirs: Directories,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ChainConfig {
    pub spec: SpecPath,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Configs {
    pub data_dir: PathBuf,
    pub db: DBConfig,
    pub chain: ChainConfig,
    pub logger: LogConfig,
    pub network: NetworkConfig,
    pub rpc: RpcConfig,
    pub block_assembler: BlockAssemblerConfig,
    pub sync: SyncConfig,
    pub tx_pool: TxPoolConfig,
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

fn find_default_config_path() -> Option<PathBuf> {
    DEFAULT_CONFIG_PATHS
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

impl Setup {
    pub(crate) fn with_configs(mut configs: Configs) -> Result<Self, Box<Error>> {
        let dirs = Directories::new(&configs.data_dir);

        if let Some(file) = configs.logger.file {
            let path = dirs.join("logs");
            configs.logger.file = Some(path.join(file));
        }

        let chain_spec = ChainSpec::read_from_file(&configs.chain.spec).map_err(|e| {
            Box::new(ConfigError::Message(format!(
                "invalid chain spec {}, {}",
                configs.chain.spec.display(),
                e
            )))
        })?;

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
        self.chain.spec = self.chain.spec.expand_path(base);
        if self.db.path.is_relative() {
            self.db.path = base.join(&self.db.path);
        }
        if self.network.path.is_relative() {
            self.network.path = base.join(&self.network.path);
        }
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
            Path::new(env!("CARGO_MANIFEST_DIR")).join("nodes_template/default.toml");
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
        name = "ckb_test_custom"

        [genesis]
        version = 0
        parent_hash = "0x0000000000000000000000000000000000000000000000000000000000000000"
        timestamp = 0
        txs_commit = "0x0000000000000000000000000000000000000000000000000000000000000000"
        txs_proposal = "0x0000000000000000000000000000000000000000000000000000000000000000"
        difficulty = "0x233"
        cellbase_id = "0x0000000000000000000000000000000000000000000000000000000000000000"
        uncles_hash = "0x0000000000000000000000000000000000000000000000000000000000000000"

        [genesis.seal]
        nonce = 233
        proof = [2, 3, 3]

        [params]
        initial_block_reward = 233
        max_block_cycles = 100000000

        [pow]
        func = "Cuckoo"

        [pow.params]
        edge_bits = 29
        cycle_length = 42

        [[system_cells]]
        path = "verify"

        [[system_cells]]
        path = "always_success"
        "#
    }

    #[test]
    fn test_load_config() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_load_config")
            .tempdir()
            .unwrap();

        let test_conifg = r#"
            [network]
            listen_addresses = ["/ip4/1.1.1.1/tcp/1"]
        "#;
        let config_path = tmp_dir.path().join("config.toml");
        write_file(&config_path, test_conifg);
        let setup = override_default_config_file(&config_path);
        assert!(setup.is_ok());
        assert_eq!(
            setup.unwrap().configs.network.listen_addresses,
            vec!["/ip4/1.1.1.1/tcp/1".parse().unwrap()]
        );
    }

    #[test]
    fn test_load_db_config() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_load_db_config")
            .tempdir()
            .unwrap();

        let test_conifg = r#"
            [db.options]
            disable_auto_compactions = "true"
            paranoid_file_checks = "true"
        "#;
        let config_path = tmp_dir.path().join("config.toml");
        write_file(&config_path, test_conifg);
        let setup = override_default_config_file(&config_path).unwrap();
        let options: Vec<(&str, &str)> = setup
            .configs
            .db
            .options
            .as_ref()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            options.contains(&("disable_auto_compactions", "true")),
            true
        );
        assert_eq!(options.contains(&("paranoid_file_checks", "true")), true);
    }

    #[test]
    fn test_custom_chain_spec_with_config() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_custom_chain_spec_with_config")
            .tempdir()
            .unwrap();

        let chain_spec_path = tmp_dir.path().join("ckb_test_custom.toml");
        let test_config = format!(
            r#"
            [chain]
            spec = "{}"
            "#,
            chain_spec_path.to_str().unwrap()
        );

        let config_path = tmp_dir.path().join("config.toml");
        write_file(&config_path, &test_config);
        write_file(&chain_spec_path, test_chain_spec());

        let setup = override_default_config_file(&config_path);
        assert!(setup.is_ok());
        assert_eq!(setup.unwrap().chain_spec.name, "ckb_test_custom");
    }

    #[test]
    fn test_testnet_chain_spec_with_config() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_testnet_chain_spec_with_config")
            .tempdir()
            .unwrap();

        let test_config = r#"
            [chain]
            spec = "testnet"
            "#;

        let config_path = tmp_dir.path().join("config.toml");
        write_file(&config_path, &test_config);

        let setup = override_default_config_file(&config_path);
        assert!(setup.is_ok());
        let setup = setup.unwrap();
        assert_eq!(setup.configs.chain.spec, SpecPath::Testnet);
        assert_eq!(setup.chain_spec.name, "ckb_testnet");
    }
}
