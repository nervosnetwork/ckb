mod app_config;
mod args;
pub mod cli;
mod exit_code;
mod sentry_config;

pub use app_config::{AppConfig, CKBAppConfig, MinerAppConfig};
pub use args::{
    ExportArgs, ImportArgs, InitArgs, MinerArgs, ProfArgs, ResetDataArgs, RunArgs, StatsArgs,
};
pub use ckb_tx_pool::BlockAssemblerConfig;
pub use exit_code::ExitCode;

use ckb_build_info::Version;
use ckb_chain_spec::{consensus::Consensus, ChainSpec};
use ckb_jsonrpc_types::ScriptHashType;
use ckb_logger::{info_target, LoggerInitGuard};
use clap::{value_t, ArgMatches, ErrorKind};
use std::path::PathBuf;

pub(crate) const LOG_TARGET_SENTRY: &str = "sentry";

pub struct Setup {
    subcommand_name: String,
    config: AppConfig,
    is_sentry_enabled: bool,
}

pub struct SetupGuard {
    #[allow(dead_code)]
    logger_guard: LoggerInitGuard,
    #[allow(dead_code)]
    sentry_guard: Option<sentry::internals::ClientInitGuard>,
}

impl Setup {
    pub fn from_matches<'m>(matches: &ArgMatches<'m>) -> Result<Setup, ExitCode> {
        let subcommand_name = match matches.subcommand_name() {
            Some(subcommand_name) => subcommand_name,
            None => {
                eprintln!("expect a subcommand");
                return Err(ExitCode::Cli);
            }
        };

        let root_dir = Self::root_dir_from_matches(matches)?;
        let config = AppConfig::load_for_subcommand(&root_dir, subcommand_name)?;
        let is_sentry_enabled = is_daemon(&subcommand_name) && config.sentry().is_enabled();

        Ok(Setup {
            subcommand_name: subcommand_name.to_string(),
            config,
            is_sentry_enabled,
        })
    }

    pub fn setup_app(&self, version: &Version) -> Result<SetupGuard, ExitCode> {
        // Initialization of logger must do before sentry, since `logger::init()` and
        // `sentry_config::init()` both registers custom panic hooks, but `logger::init()`
        // replaces all hooks previously registered.
        let mut logger_config = self.config.logger().to_owned();
        if logger_config.emit_sentry_breadcrumbs.is_none() {
            logger_config.emit_sentry_breadcrumbs = Some(self.is_sentry_enabled);
        }
        let logger_guard = ckb_logger::init(logger_config)?;

        let sentry_guard = if self.is_sentry_enabled {
            let sentry_config = self.config.sentry();

            info_target!(
                crate::LOG_TARGET_SENTRY,
                "**Notice**: \
                 The ckb process will send stack trace to sentry on Rust panics. \
                 This is enabled by default before mainnet, which can be opted out by setting \
                 the option `dsn` to empty in the config file. The DSN is now {}",
                sentry_config.dsn
            );

            let guard = sentry_config.init(&version);

            sentry::configure_scope(|scope| {
                scope.set_tag("subcommand", &self.subcommand_name);
            });

            Some(guard)
        } else {
            info_target!(crate::LOG_TARGET_SENTRY, "sentry is disabled");
            None
        };

        Ok(SetupGuard {
            logger_guard,
            sentry_guard,
        })
    }

    pub fn run<'m>(self, matches: &ArgMatches<'m>) -> Result<RunArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;

        Ok(RunArgs {
            config,
            consensus,
            block_assembler_advanced: matches.is_present(cli::ARG_BA_ADVANCED),
        })
    }

    pub fn miner<'m>(self, matches: &ArgMatches<'m>) -> Result<MinerArgs, ExitCode> {
        let spec = self.chain_spec()?;
        let memory_tracker = self.config.memory_tracker().to_owned();
        let config = self.config.into_miner()?;
        let pow_engine = spec.pow_engine();
        let limit = match value_t!(matches, cli::ARG_LIMIT, u128) {
            Ok(l) => l,
            Err(ref e) if e.kind == ErrorKind::ArgumentNotFound => 0,
            Err(e) => {
                return Err(e.into());
            }
        };

        Ok(MinerArgs {
            pow_engine,
            config: config.miner,
            memory_tracker,
            limit,
        })
    }

    pub fn prof<'m>(self, matches: &ArgMatches<'m>) -> Result<ProfArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let tmp_target = value_t!(matches, cli::ARG_TMP_TARGET, PathBuf)?;
        let from = value_t!(matches, cli::ARG_FROM, u64)?;
        let to = value_t!(matches, cli::ARG_TO, u64)?;

        Ok(ProfArgs {
            config,
            consensus,
            tmp_target,
            from,
            to,
        })
    }

    pub fn stats<'m>(self, matches: &ArgMatches<'m>) -> Result<StatsArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        // There are two types of errors,
        // parse failures and those where the argument wasn't present
        let from = match value_t!(matches, cli::ARG_FROM, u64) {
            Ok(from) => Some(from),
            Err(ref e) if e.kind == ErrorKind::ArgumentNotFound => None,
            Err(e) => {
                return Err(e.into());
            }
        };
        let to = match value_t!(matches, cli::ARG_TO, u64) {
            Ok(to) => Some(to),
            Err(ref e) if e.kind == ErrorKind::ArgumentNotFound => None,
            Err(e) => {
                return Err(e.into());
            }
        };

        Ok(StatsArgs {
            config,
            consensus,
            from,
            to,
        })
    }

    pub fn import<'m>(self, matches: &ArgMatches<'m>) -> Result<ImportArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let source = value_t!(matches.value_of(cli::ARG_SOURCE), PathBuf)?;

        Ok(ImportArgs {
            config,
            consensus,
            source,
        })
    }

    pub fn export<'m>(self, matches: &ArgMatches<'m>) -> Result<ExportArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let target = value_t!(matches.value_of(cli::ARG_TARGET), PathBuf)?;

        Ok(ExportArgs {
            config,
            consensus,
            target,
        })
    }

    pub fn init<'m>(matches: &ArgMatches<'m>) -> Result<InitArgs, ExitCode> {
        if matches.is_present("list-specs") {
            eprintln!(
                "Deprecated: Option `--list-specs` is deprecated, use `--list-chains` instead"
            );
        }
        if matches.is_present("spec") {
            eprintln!("Deprecated: Option `--spec` is deprecated, use `--chain` instead");
        }
        if matches.is_present("export-specs") {
            eprintln!("Deprecated: Option `--export-specs` is deprecated");
        }

        let root_dir = Self::root_dir_from_matches(matches)?;
        let list_chains =
            matches.is_present(cli::ARG_LIST_CHAINS) || matches.is_present("list-specs");
        let interactive = matches.is_present(cli::ARG_INTERACTIVE);
        let force = matches.is_present(cli::ARG_FORCE);
        let chain = if matches.occurrences_of(cli::ARG_CHAIN) > 0 || !matches.is_present("spec") {
            matches.value_of(cli::ARG_CHAIN).unwrap().to_string()
        } else {
            matches.value_of("spec").unwrap().to_string()
        };
        let rpc_port = matches.value_of(cli::ARG_RPC_PORT).unwrap().to_string();
        let p2p_port = matches.value_of(cli::ARG_P2P_PORT).unwrap().to_string();
        let (log_to_file, log_to_stdout) = match matches.value_of(cli::ARG_LOG_TO) {
            Some("file") => (true, false),
            Some("stdout") => (false, true),
            Some("both") => (true, true),
            _ => unreachable!(),
        };

        let block_assembler_code_hash = matches.value_of(cli::ARG_BA_CODE_HASH).map(str::to_string);
        let block_assembler_args: Vec<_> = matches
            .values_of(cli::ARG_BA_ARG)
            .unwrap_or_default()
            .map(str::to_string)
            .collect();
        let block_assembler_hash_type = matches
            .value_of(cli::ARG_BA_HASH_TYPE)
            .and_then(|hash_type| serde_plain::from_str::<ScriptHashType>(hash_type).ok())
            .unwrap();
        let block_assembler_message = matches.value_of(cli::ARG_BA_MESSAGE).map(str::to_string);

        let import_spec = matches.value_of(cli::ARG_IMPORT_SPEC).map(str::to_string);

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
        })
    }

    pub fn reset_data<'m>(self, matches: &ArgMatches<'m>) -> Result<ResetDataArgs, ExitCode> {
        let config = self.config.into_ckb()?;
        let data_dir = config.data_dir;
        let db_path = config.db.path;
        let indexer_db_path = config.indexer.db.path;
        let network_config = config.network;
        let network_dir = network_config.path.clone();
        let network_peer_store_path = network_config.peer_store_path();
        let network_secret_key_path = network_config.secret_key_path();
        let logs_dir = config
            .logger
            .file
            .and_then(|path| path.parent().map(|dir| dir.to_path_buf()));

        let force = matches.is_present(cli::ARG_FORCE);
        let all = matches.is_present(cli::ARG_ALL);
        let database = matches.is_present(cli::ARG_DATABASE);
        let indexer = matches.is_present(cli::ARG_INDEXER);
        let network = matches.is_present(cli::ARG_NETWORK);
        let network_peer_store = matches.is_present(cli::ARG_NETWORK_PEER_STORE);
        let network_secret_key = matches.is_present(cli::ARG_NETWORK_SECRET_KEY);
        let logs = matches.is_present(cli::ARG_LOGS);

        Ok(ResetDataArgs {
            force,
            all,
            database,
            indexer,
            network,
            network_peer_store,
            network_secret_key,
            logs,
            data_dir,
            db_path,
            indexer_db_path,
            network_dir,
            network_peer_store_path,
            network_secret_key_path,
            logs_dir,
        })
    }

    pub fn root_dir_from_matches<'m>(matches: &ArgMatches<'m>) -> Result<PathBuf, ExitCode> {
        let config_dir = match matches.value_of(cli::ARG_CONFIG_DIR) {
            Some(arg_config_dir) => PathBuf::from(arg_config_dir),
            None => ::std::env::current_dir()?,
        };
        std::fs::create_dir_all(&config_dir)?;
        Ok(config_dir)
    }

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

    fn consensus(&self) -> Result<Consensus, ExitCode> {
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
}

fn is_daemon(subcommand_name: &str) -> bool {
    match subcommand_name {
        cli::CMD_RUN => true,
        cli::CMD_MINER => true,
        _ => false,
    }
}

fn consensus_from_spec(spec: &ChainSpec) -> Result<Consensus, ExitCode> {
    spec.build_consensus().map_err(|err| {
        eprintln!("chainspec error: {}", err);
        ExitCode::Config
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::CMD_STATS;
    use clap::{App, AppSettings};

    #[test]
    fn stats_args() {
        let app = App::new("stats_args_test")
            .setting(AppSettings::SubcommandRequiredElseHelp)
            .subcommand(cli::stats());

        let stats = app.clone().get_matches_from_safe(vec!["", CMD_STATS]);
        assert!(stats.is_ok());

        let stats = app
            .clone()
            .get_matches_from_safe(vec!["", CMD_STATS, "--from", "10"]);
        assert!(stats.is_ok());

        let stats = app
            .clone()
            .get_matches_from_safe(vec!["", CMD_STATS, "--to", "100"]);
        assert!(stats.is_ok());

        let stats = app
            .clone()
            .get_matches_from_safe(vec!["", CMD_STATS, "--from", "10", "--to", "100"]);
        assert!(stats.is_ok());
    }
}
