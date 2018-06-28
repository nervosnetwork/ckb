use chain::Config as ChainConfig;
use clap;
use config::{Config, ConfigError, File, FileFormat};
use dir::{default_base_path, Directories};
use logger::Config as LogConfig;
use miner::Config as MinerConfig;
use network::Config as NetworkConfig;
use rpc::Config as RpcConfig;
use std::env;

#[derive(Clone, Debug)]
pub struct Spec {
    pub configs: Configs,
    pub dirs: Directories,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Configs {
    pub logger: LogConfig,
    pub network: NetworkConfig,
    pub rpc: RpcConfig,
    pub chain: ChainConfig,
    pub miner: MinerConfig,
}

impl Spec {
    pub fn new(matches: &clap::ArgMatches) -> Result<Self, ConfigError> {
        let data_path = matches
            .value_of("data-dir")
            .map(Into::into)
            .unwrap_or_else(default_base_path);
        let dirs = Directories::new(&data_path);

        let mut c = Config::new();
        c.merge(File::from_str(
            include_str!("config/default.toml"),
            FileFormat::Toml,
        ))?;
        let env = env::var("NERVOS_ENV").unwrap_or_else(|_| "development".into());
        c.merge(File::with_name(data_path.join(env).to_str().unwrap()).required(false))?;
        c.try_into().map(|configs| Spec { configs, dirs })
    }
}
