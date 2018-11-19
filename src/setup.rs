use chain_spec::{ChainSpec, SpecType};
use clap;
use config_tool::{Config as ConfigTool, File, FileFormat};
use dir::{default_base_path, Directories};
use logger::Config as LogConfig;
use miner::Config as MinerConfig;
use network::Config as NetworkConfig;
use rpc::Config as RpcConfig;
use std::error::Error;
use sync::Config as SyncConfig;

#[derive(Clone, Debug)]
pub struct Setup {
    pub configs: Configs,
    pub chain_spec: ChainSpec,
    pub dirs: Directories,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CKB {
    chain: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Configs {
    pub ckb: CKB,
    pub logger: LogConfig,
    pub network: NetworkConfig,
    pub rpc: RpcConfig,
    pub miner: MinerConfig,
    pub sync: SyncConfig,
}

pub const DEFAULT_CONFIG_FILENAME: &str = "config.toml";

impl Setup {
    pub fn new(matches: &clap::ArgMatches) -> Result<Self, Box<Error>> {
        let data_path = matches
            .value_of("data-dir")
            .map(Into::into)
            .unwrap_or_else(default_base_path);
        let dirs = Directories::new(&data_path);

        let mut config_tool = ConfigTool::new();
        config_tool.merge(File::from_str(
            include_str!("config/default.toml"),
            FileFormat::Toml,
        ))?;

        // if config arg is present, open and load it as required,
        // otherwise load the default config from data-dir
        if let Some(config_path) = matches.value_of("config") {
            config_tool.merge(File::with_name(config_path).required(true))?;
        } else {
            config_tool.merge(
                File::with_name(data_path.join(DEFAULT_CONFIG_FILENAME).to_str().unwrap())
                    .required(false),
            )?;
        }

        let mut configs: Configs = config_tool.try_into()?;
        if let Some(file) = configs.logger.file {
            let mut path = dirs.join("logs");
            path.push(file);
            configs.logger.file = Some(path.to_str().unwrap().to_string());
        }
        if configs.network.net_config_path.is_none() {
            configs.network.net_config_path =
                Some(dirs.join("network").to_string_lossy().to_string());
        }
        if let Some(file) = configs.miner.ethash_path {
            let mut path = dirs.join("miner");
            path.push(file);
            configs.miner.ethash_path = Some(path.to_str().unwrap().to_string());
        }

        //run with the --chain option or with a config file specifying chain = "path" under [ckb]
        let spec_type: SpecType = matches
            .value_of("chain")
            .unwrap_or_else(|| &configs.ckb.chain)
            .parse()?;
        let chain_spec = spec_type.load_spec()?;

        Ok(Setup {
            configs,
            chain_spec,
            dirs,
        })
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;
    use tempdir::TempDir;

    fn write_file<P: AsRef<Path>>(file: P, content: &str) {
        let mut file = File::create(file).expect("test dir clean");
        file.write_all(content.as_bytes())
            .expect("write test content");;
    }

    fn test_chain_spec() -> &'static str {
        r#"
        name: "ckb_test_custom"
        genesis: 
            seal: 
                nonce: 233
                mix_hash: "0x0000000000000000000000000000000000000000000000000000000000000233"
            version: 0
            parent_hash: "0x0000000000000000000000000000000000000000000000000000000000000233"
            timestamp: 0
            txs_commit: "0x0000000000000000000000000000000000000000000000000000000000000233"
            difficulty: "0x233"
        params: 
            initial_block_reward: 233
            min_difficulty: "0x233"
        "#
    }

    #[test]
    fn test_data_dir() {
        let tmp_dir = TempDir::new("test_data_dir").unwrap();
        let data_path = tmp_dir.path().to_str().unwrap();
        let arg_vec = vec!["ckb", "--data-dir", data_path];
        let yaml = load_yaml!("cli/app.yml");
        let matches = clap::App::from_yaml(yaml).get_matches_from(arg_vec);
        let setup = Setup::new(&matches);
        assert!(setup.is_ok());
        assert_eq!(setup.unwrap().dirs.base, tmp_dir.path());
    }

    #[test]
    fn test_load_config() {
        let tmp_dir = TempDir::new("test_specify_config").unwrap();
        let data_path = tmp_dir.path().to_str().unwrap();

        let test_conifg = r#"[network]
                             listen_address = "1.1.1.1:1""#;
        let config_path = tmp_dir.path().join("config.toml");
        write_file(config_path, test_conifg);
        let arg_vec = vec!["ckb", "--data-dir", data_path];
        let yaml = load_yaml!("cli/app.yml");
        let matches = clap::App::from_yaml(yaml).get_matches_from(arg_vec);
        let setup = Setup::new(&matches);
        assert!(setup.is_ok());
        assert_eq!(
            setup.unwrap().configs.network.listen_address,
            "1.1.1.1:1".parse().ok()
        );
    }

    #[test]
    fn test_specify_config() {
        let tmp_dir = TempDir::new("test_specify_config").unwrap();
        let data_path = tmp_dir.path().to_str().unwrap();

        let test_conifg = r#"[network]
                             listen_address = "1.1.1.1:1""#;
        let config_path = tmp_dir.path().join("specify.toml");
        write_file(&config_path, test_conifg);
        let arg_vec = vec![
            "ckb",
            "--data-dir",
            data_path,
            "--config",
            config_path.to_str().unwrap(),
        ];
        let yaml = load_yaml!("cli/app.yml");
        let matches = clap::App::from_yaml(yaml).get_matches_from(arg_vec);
        let setup = Setup::new(&matches);
        assert!(setup.is_ok());
        assert_eq!(
            setup.unwrap().configs.network.listen_address,
            "1.1.1.1:1".parse().ok()
        );
    }

    #[test]
    fn test_custom_chain_spec_with_config() {
        let tmp_dir = TempDir::new("test_custom_chain_spec").unwrap();
        let data_path = tmp_dir.path().to_str().unwrap();
        let arg_vec = vec!["ckb", "--data-dir", data_path];
        let yaml = load_yaml!("cli/app.yml");

        let chain_spec_path = tmp_dir.path().join("ckb_test_custom.toml");
        let test_conifg = format!("[ckb]\nchain = \"{}\"", chain_spec_path.to_str().unwrap());
        let config_path = tmp_dir.path().join("config.toml");
        write_file(&config_path, &test_conifg);
        write_file(&chain_spec_path, test_chain_spec());

        let matches = clap::App::from_yaml(yaml).get_matches_from(arg_vec);
        let setup = Setup::new(&matches);
        assert!(setup.is_ok());
        assert_eq!(setup.unwrap().chain_spec.name, "ckb_test_custom");
    }

    #[test]
    fn test_custom_chain_spec_with_arg() {
        let tmp_dir = TempDir::new("test_custom_chain_spec").unwrap();
        let data_path = tmp_dir.path().to_str().unwrap();

        let chain_spec_path = tmp_dir.path().join("ckb_test_custom.toml");
        let arg_vec = vec![
            "ckb",
            "--data-dir",
            data_path,
            "--chain",
            chain_spec_path.to_str().unwrap(),
        ];
        write_file(&chain_spec_path, test_chain_spec());

        let yaml = load_yaml!("cli/app.yml");
        let matches = clap::App::from_yaml(yaml).get_matches_from(arg_vec);
        let setup = Setup::new(&matches);
        assert!(setup.is_ok());
        assert_eq!(setup.unwrap().chain_spec.name, "ckb_test_custom");
    }
}
