pub use ckb_app_config::{AppConfig, CKBAppConfig, ExitCode, MinerAppConfig};
pub use ckb_miner::BlockAssemblerConfig;

use ckb_build_info::Version;
use ckb_chain_spec::{consensus::Consensus, ChainSpec};
use ckb_instrument::Format;
use ckb_logger::{info_target, LoggerInitGuard};
use clap::{value_t, ArgMatches, ErrorKind};
use std::path::PathBuf;

use crate::{
    args::{ExportArgs, ImportArgs, InitArgs, MinerArgs, ProfArgs, RunArgs, StatsArgs},
    cli_cmds,
};

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
        let logger_guard = ckb_logger::init(self.config.logger().to_owned())?;

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

    pub fn run(self) -> Result<RunArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;

        Ok(RunArgs { config, consensus })
    }

    pub fn miner(self) -> Result<MinerArgs, ExitCode> {
        let spec = self.chain_spec()?;
        let config = self.config.into_miner()?;
        let pow_engine = spec.pow_engine();

        Ok(MinerArgs {
            pow_engine,
            config: config.miner,
        })
    }

    pub fn prof<'m>(self, matches: &ArgMatches<'m>) -> Result<ProfArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let from = value_t!(matches, cli_cmds::ARG_FROM, u64)?;
        let to = value_t!(matches, cli_cmds::ARG_TO, u64)?;

        Ok(ProfArgs {
            config,
            consensus,
            from,
            to,
        })
    }

    pub fn stats<'m>(self, matches: &ArgMatches<'m>) -> Result<StatsArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        // There are two types of errors,
        // parse failures and those where the argument wasn't present
        let from = match value_t!(matches, cli_cmds::ARG_FROM, u64) {
            Ok(from) => Some(from),
            Err(ref e) if e.kind == ErrorKind::ArgumentNotFound => None,
            Err(e) => {
                return Err(e.into());
            }
        };
        let to = match value_t!(matches, cli_cmds::ARG_TO, u64) {
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
        let format = value_t!(matches.value_of(cli_cmds::ARG_FORMAT), Format)?;
        let source = value_t!(matches.value_of(cli_cmds::ARG_SOURCE), PathBuf)?;

        Ok(ImportArgs {
            config,
            consensus,
            format,
            source,
        })
    }

    pub fn export<'m>(self, matches: &ArgMatches<'m>) -> Result<ExportArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let format = value_t!(matches.value_of(cli_cmds::ARG_FORMAT), Format)?;
        let target = value_t!(matches.value_of(cli_cmds::ARG_TARGET), PathBuf)?;

        Ok(ExportArgs {
            config,
            consensus,
            format,
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
            matches.is_present(cli_cmds::ARG_LIST_CHAINS) || matches.is_present("list-specs");
        let force = matches.is_present(cli_cmds::ARG_FORCE);
        let chain =
            if matches.occurrences_of(cli_cmds::ARG_CHAIN) > 0 || !matches.is_present("spec") {
                matches.value_of(cli_cmds::ARG_CHAIN).unwrap().to_string()
            } else {
                matches.value_of("spec").unwrap().to_string()
            };
        let rpc_port = matches
            .value_of(cli_cmds::ARG_RPC_PORT)
            .unwrap()
            .to_string();
        let p2p_port = matches
            .value_of(cli_cmds::ARG_P2P_PORT)
            .unwrap()
            .to_string();
        let (log_to_file, log_to_stdout) = match matches.value_of(cli_cmds::ARG_LOG_TO) {
            Some("file") => (true, false),
            Some("stdout") => (false, true),
            Some("both") => (true, true),
            _ => unreachable!(),
        };

        let block_assembler_code_hash = matches
            .value_of(cli_cmds::ARG_BA_CODE_HASH)
            .map(str::to_string);
        let block_assembler_args: Vec<_> = matches
            .values_of(cli_cmds::ARG_BA_ARG)
            .unwrap_or_default()
            .map(str::to_string)
            .collect();
        let block_assembler_data = matches.value_of(cli_cmds::ARG_BA_DATA).map(str::to_string);

        Ok(InitArgs {
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
            block_assembler_data,
        })
    }

    pub fn root_dir_from_matches<'m>(matches: &ArgMatches<'m>) -> Result<PathBuf, ExitCode> {
        let config_dir = match matches.value_of(cli_cmds::ARG_CONFIG_DIR) {
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
        cli_cmds::CMD_RUN => true,
        cli_cmds::CMD_MINER => true,
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
    use crate::cli_cmds::CMD_STATS;
    use clap::{App, AppSettings};

    #[test]
    fn stats_args() {
        let app = App::new("stats_args_test")
            .setting(AppSettings::SubcommandRequiredElseHelp)
            .subcommand(cli_cmds::stats());

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
