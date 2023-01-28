//! CKB command line arguments and config options.
mod app_config;
mod args;
pub mod cli;
mod configs;
mod exit_code;
pub(crate) mod legacy;
#[cfg(feature = "with_sentry")]
mod sentry_config;

#[cfg(test)]
mod tests;

pub use app_config::{
    AppConfig, CKBAppConfig, ChainConfig, LogConfig, MetricsConfig, MinerAppConfig,
};
pub use args::{
    ExportArgs, ImportArgs, InitArgs, MigrateArgs, MinerArgs, PeerIDArgs, RepairArgs, ReplayArgs,
    ResetDataArgs, RunArgs, StatsArgs,
};
pub use configs::*;
pub use exit_code::ExitCode;
#[cfg(feature = "with_sentry")]
pub use sentry_config::SentryConfig;
pub use url::Url;

use ckb_chain_spec::{consensus::Consensus, ChainSpec};
use ckb_jsonrpc_types::ScriptHashType;
use ckb_types::{u256, H256, U256};
use clap::ArgMatches;
use std::{path::PathBuf, str::FromStr};

// 500_000 total difficulty
const MIN_CHAIN_WORK_500K: U256 = u256!("0x3314412053c82802a7");

/// A struct including all the information to start the ckb process.
pub struct Setup {
    /// Subcommand name.
    ///
    /// For example, this is set to `run` when ckb is executed with `ckb run`.
    pub subcommand_name: String,
    /// The config file for the current subcommand.
    pub config: AppConfig,
    /// Whether sentry is enabled.
    #[cfg(feature = "with_sentry")]
    pub is_sentry_enabled: bool,
}

impl Setup {
    /// Boots the ckb process by parsing the command line arguments and loading the config file.
    pub fn from_matches(
        bin_name: String,
        subcommand_name: &str,
        matches: &ArgMatches,
    ) -> Result<Setup, ExitCode> {
        let root_dir = Self::root_dir_from_matches(matches)?;
        let mut config = AppConfig::load_for_subcommand(root_dir, subcommand_name)?;
        config.set_bin_name(bin_name);
        #[cfg(feature = "with_sentry")]
        let is_sentry_enabled = is_daemon(subcommand_name) && config.sentry().is_enabled();

        Ok(Setup {
            subcommand_name: subcommand_name.to_string(),
            config,
            #[cfg(feature = "with_sentry")]
            is_sentry_enabled,
        })
    }

    /// Executes `ckb run`.
    pub fn run(self, matches: &ArgMatches) -> Result<RunArgs, ExitCode> {
        let consensus = self.consensus()?;
        let chain_spec_hash = self.chain_spec()?.hash;
        let mut config = self.config.into_ckb()?;

        let mainnet_genesis = ckb_chain_spec::ChainSpec::load_from(
            &ckb_resource::Resource::bundled("specs/mainnet.toml".to_string()),
        )
        .expect("load mainnet spec fail")
        .build_genesis()
        .expect("build mainnet genesis fail");
        config.network.sync.min_chain_work =
            if consensus.genesis_block.hash() == mainnet_genesis.hash() {
                MIN_CHAIN_WORK_500K
            } else {
                u256!("0x0")
            };

        config.network.sync.assume_valid_target = matches
            .get_one::<String>(cli::ARG_ASSUME_VALID_TARGET)
            .and_then(|s| H256::from_str(&s[2..]).ok());

        Ok(RunArgs {
            config,
            consensus,
            block_assembler_advanced: matches.get_flag(cli::ARG_BA_ADVANCED),
            skip_chain_spec_check: matches.get_flag(cli::ARG_SKIP_CHAIN_SPEC_CHECK),
            overwrite_chain_spec: matches.get_flag(cli::ARG_OVERWRITE_CHAIN_SPEC),
            chain_spec_hash,
            indexer: matches.get_flag(cli::ARG_INDEXER),
        })
    }

    /// `migrate` subcommand has one `flags` arg, trigger this arg with "--check"
    pub fn migrate(self, matches: &ArgMatches) -> Result<MigrateArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let check = matches.get_flag(cli::ARG_MIGRATE_CHECK);
        let force = matches.get_flag(cli::ARG_FORCE);

        Ok(MigrateArgs {
            config,
            consensus,
            check,
            force,
        })
    }

    /// `db-repair` subcommand
    pub fn db_repair(self, _matches: &ArgMatches) -> Result<RepairArgs, ExitCode> {
        let config = self.config.into_ckb()?;

        Ok(RepairArgs { config })
    }

    /// Executes `ckb miner`.
    pub fn miner(self, matches: &ArgMatches) -> Result<MinerArgs, ExitCode> {
        let spec = self.chain_spec()?;
        let memory_tracker = self.config.memory_tracker().to_owned();
        let config = self.config.into_miner()?;
        let pow_engine = spec.pow_engine();
        let limit = *matches
            .get_one::<u128>(cli::ARG_LIMIT)
            .expect("has default value");

        Ok(MinerArgs {
            pow_engine,
            config: config.miner,
            memory_tracker,
            limit,
        })
    }

    /// Executes `ckb replay`.
    pub fn replay(self, matches: &ArgMatches) -> Result<ReplayArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let tmp_target = matches
            .get_one::<PathBuf>(cli::ARG_TMP_TARGET)
            .ok_or_else(|| {
                eprintln!("Args Error: {:?} no found", cli::ARG_TMP_TARGET);
                ExitCode::Cli
            })?
            .clone();
        let profile = if matches.get_flag(cli::ARG_PROFILE) {
            let from = matches.get_one::<u64>(cli::ARG_FROM).cloned();
            let to = matches.get_one::<u64>(cli::ARG_TO).cloned();
            Some((from, to))
        } else {
            None
        };
        let sanity_check = matches.get_flag(cli::ARG_SANITY_CHECK);
        let full_verification = matches.get_flag(cli::ARG_FULL_VERIFICATION);
        Ok(ReplayArgs {
            config,
            consensus,
            tmp_target,
            profile,
            sanity_check,
            full_verification,
        })
    }

    /// Executes `ckb stats`.
    pub fn stats(self, matches: &ArgMatches) -> Result<StatsArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;

        let from = matches.get_one::<u64>(cli::ARG_FROM).cloned();
        let to = matches.get_one::<u64>(cli::ARG_TO).cloned();

        Ok(StatsArgs {
            config,
            consensus,
            from,
            to,
        })
    }

    /// Executes `ckb import`.
    pub fn import(self, matches: &ArgMatches) -> Result<ImportArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let source = matches
            .get_one::<PathBuf>(cli::ARG_SOURCE)
            .ok_or_else(|| {
                eprintln!("Args Error: {:?} no found", cli::ARG_SOURCE);
                ExitCode::Cli
            })?
            .clone();

        Ok(ImportArgs {
            config,
            consensus,
            source,
        })
    }

    /// Executes `ckb export`.
    pub fn export(self, matches: &ArgMatches) -> Result<ExportArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let target = matches
            .get_one::<PathBuf>(cli::ARG_TARGET)
            .ok_or_else(|| {
                eprintln!("Args Error: {:?} no found", cli::ARG_TARGET);
                ExitCode::Cli
            })?
            .clone();

        Ok(ExportArgs {
            config,
            consensus,
            target,
        })
    }

    /// Executes `ckb init`.
    pub fn init(matches: &ArgMatches) -> Result<InitArgs, ExitCode> {
        if matches.contains_id("list-specs") {
            eprintln!(
                "Deprecated: Option `--list-specs` is deprecated, use `--list-chains` instead"
            );
        }
        if matches.contains_id("spec") {
            eprintln!("Deprecated: Option `--spec` is deprecated, use `--chain` instead");
        }
        if matches.contains_id("export-specs") {
            eprintln!("Deprecated: Option `--export-specs` is deprecated");
        }

        let root_dir = Self::root_dir_from_matches(matches)?;
        let list_chains =
            matches.get_flag(cli::ARG_LIST_CHAINS) || matches.contains_id("list-specs");
        let interactive = matches.get_flag(cli::ARG_INTERACTIVE);
        let force = matches.get_flag(cli::ARG_FORCE);
        let chain = if !matches.contains_id("spec") {
            matches
                .get_one::<String>(cli::ARG_CHAIN)
                .expect("has default value")
                .to_string()
        } else {
            matches.get_one::<String>("spec").unwrap().to_string()
        };
        let rpc_port = matches
            .get_one::<String>(cli::ARG_RPC_PORT)
            .expect("has default value")
            .to_string();
        let p2p_port = matches
            .get_one::<String>(cli::ARG_P2P_PORT)
            .expect("has default value")
            .to_string();
        let (log_to_file, log_to_stdout) = match matches
            .get_one::<String>(cli::ARG_LOG_TO)
            .map(|s| s.as_str())
        {
            Some("file") => (true, false),
            Some("stdout") => (false, true),
            Some("both") => (true, true),
            _ => unreachable!(),
        };

        let block_assembler_code_hash = matches.get_one::<String>(cli::ARG_BA_CODE_HASH).cloned();
        let block_assembler_args: Vec<_> = matches
            .get_many::<String>(cli::ARG_BA_ARG)
            .unwrap_or_default()
            .map(|a| a.to_owned())
            .collect();
        let block_assembler_hash_type = matches
            .get_one::<String>(cli::ARG_BA_HASH_TYPE)
            .and_then(|hash_type| serde_plain::from_str::<ScriptHashType>(hash_type).ok())
            .expect("has default value");
        let block_assembler_message = matches.get_one::<String>(cli::ARG_BA_MESSAGE).cloned();

        let import_spec = matches.get_one::<String>(cli::ARG_IMPORT_SPEC).cloned();

        let customize_spec = {
            let genesis_message = matches.get_one::<String>(cli::ARG_GENESIS_MESSAGE).cloned();
            args::CustomizeSpec { genesis_message }
        };

        Ok(InitArgs {
            interactive,
            root_dir,
            chain,
            rpc_port,
            p2p_port,
            list_chains,
            force,
            log_to_file,
            log_to_stdout,
            block_assembler_code_hash,
            block_assembler_args,
            block_assembler_hash_type,
            block_assembler_message,
            import_spec,
            customize_spec,
        })
    }

    /// Executes `ckb reset-data`.
    pub fn reset_data(self, matches: &ArgMatches) -> Result<ResetDataArgs, ExitCode> {
        let config = self.config.into_ckb()?;
        let data_dir = config.data_dir;
        let db_path = config.db.path;
        let network_config = config.network;
        let network_dir = network_config.path.clone();
        let network_peer_store_path = network_config.peer_store_path();
        let network_secret_key_path = network_config.secret_key_path();
        let logs_dir = Some(config.logger.log_dir);

        let force = matches.get_flag(cli::ARG_FORCE);
        let all = matches.get_flag(cli::ARG_ALL);
        let database = matches.get_flag(cli::ARG_DATABASE);
        let network = matches.get_flag(cli::ARG_NETWORK);
        let network_peer_store = matches.get_flag(cli::ARG_NETWORK_PEER_STORE);
        let network_secret_key = matches.get_flag(cli::ARG_NETWORK_SECRET_KEY);
        let logs = matches.get_flag(cli::ARG_LOGS);

        Ok(ResetDataArgs {
            force,
            all,
            database,
            network,
            network_peer_store,
            network_secret_key,
            logs,
            data_dir,
            db_path,
            network_dir,
            network_peer_store_path,
            network_secret_key_path,
            logs_dir,
        })
    }

    /// Resolves the root directory for ckb from the command line arguments.
    pub fn root_dir_from_matches(matches: &ArgMatches) -> Result<PathBuf, ExitCode> {
        let config_dir = match matches.get_one::<String>(cli::ARG_CONFIG_DIR) {
            Some(arg_config_dir) => PathBuf::from(arg_config_dir),
            None => ::std::env::current_dir()?,
        };
        std::fs::create_dir_all(&config_dir)?;
        Ok(config_dir)
    }

    /// Loads the chain spec.
    #[cfg(feature = "with_sentry")]
    fn chain_spec(&self) -> Result<ChainSpec, ExitCode> {
        let result = self.config.chain_spec();
        if let Ok(spec) = &result {
            if self.is_sentry_enabled {
                sentry::configure_scope(|scope| {
                    scope.set_tag("spec.name", &spec.name);
                    scope.set_tag("spec.pow", &spec.pow);
                });
            }
        }

        result
    }

    #[cfg(not(feature = "with_sentry"))]
    fn chain_spec(&self) -> Result<ChainSpec, ExitCode> {
        self.config.chain_spec()
    }

    /// Gets the consensus.
    #[cfg(feature = "with_sentry")]
    pub fn consensus(&self) -> Result<Consensus, ExitCode> {
        let result = consensus_from_spec(&self.chain_spec()?);

        if let Ok(consensus) = &result {
            if self.is_sentry_enabled {
                sentry::configure_scope(|scope| {
                    scope.set_tag("genesis", consensus.genesis_hash());
                });
            }
        }

        result
    }

    /// Gets the consensus.
    #[cfg(not(feature = "with_sentry"))]
    pub fn consensus(&self) -> Result<Consensus, ExitCode> {
        consensus_from_spec(&self.chain_spec()?)
    }

    /// Gets the network peer id by reading the network secret key.
    pub fn peer_id(matches: &ArgMatches) -> Result<PeerIDArgs, ExitCode> {
        let path = matches
            .get_one::<String>(cli::ARG_SECRET_PATH)
            .expect("required on command line");
        match read_secret_key(path.into()) {
            Ok(Some(key)) => Ok(PeerIDArgs {
                peer_id: key.peer_id(),
            }),
            Err(_) => Err(ExitCode::Failure),
            Ok(None) => Err(ExitCode::IO),
        }
    }

    /// Generates the network secret key.
    pub fn gen(matches: &ArgMatches) -> Result<(), ExitCode> {
        let path = matches
            .get_one::<String>(cli::ARG_SECRET_PATH)
            .expect("required on command line");
        configs::write_secret_to_file(&configs::generate_random_key(), path.into())
            .map_err(|_| ExitCode::IO)
    }
}

#[cfg(feature = "with_sentry")]
fn is_daemon(subcommand_name: &str) -> bool {
    matches!(subcommand_name, cli::CMD_RUN | cli::CMD_MINER)
}

fn consensus_from_spec(spec: &ChainSpec) -> Result<Consensus, ExitCode> {
    spec.build_consensus().map_err(|err| {
        eprintln!("chainspec error: {}", err);
        ExitCode::Config
    })
}
