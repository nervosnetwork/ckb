use clap;
use cli::{Signer, Spec, TemplatesExt};
use crypto::rsa::Rsa;
use dir::{default_base_path, Directories};
use logger::Config as LogConfig;
use network::Config as NetworkConfig;
use rpc::Config as RpcConfig;

const CONFIG_FILE: &str = "config.toml";
const SIGNER_FILE: &str = "signer.toml";
const RSA_FILE: &str = "rsa.toml";
const PEERS: &str = "peers.json";

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub logger: LogConfig,
    pub signer: Signer,
    pub network: NetworkConfig,
    pub dirs: Directories,
    pub rpc: RpcConfig,
}

impl Config {
    pub fn parse(matches: &clap::ArgMatches) -> Config {
        let dirs = Self::parse_data_path(matches);
        let spec = Spec::load_or_write_default(&dirs.base.join(CONFIG_FILE)).expect("load spec");

        let signer =
            Signer::load_or_write_default(&dirs.signer.join(SIGNER_FILE)).expect("load signer");

        let rsa = Rsa::load_or_write_default(&dirs.keys.join(RSA_FILE)).expect("load rsa");

        let Spec {
            network,
            logger,
            rpc,
        } = spec;

        let network = NetworkConfig {
            private_key: rsa.privkey_pkcs8,
            public_key: rsa.pubkey_der,
            listen_addr: network.listen_addr,
            bootstrap_nodes: network.bootstrap_nodes,
            peer_path: dirs.base.join(PEERS),
        };

        let rpc = RpcConfig {
            listen_addr: rpc.listen_addr,
        };

        Config {
            dirs,
            signer,
            logger,
            network,
            rpc,
        }
    }

    fn parse_data_path(matches: &clap::ArgMatches) -> Directories {
        let data_path = matches
            .value_of("data-dir")
            .map(Into::into)
            .unwrap_or_else(default_base_path);
        let data_dir = Directories::new(&data_path);
        data_dir.create_dirs().expect("Create data dir");
        data_dir
    }

    pub fn logger_config(&self) -> LogConfig {
        self.logger.clone()
    }
}
