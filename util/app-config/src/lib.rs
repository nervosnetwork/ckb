mod app_config;
mod args;
pub mod cli;
mod exit_code;
mod sentry_config;

pub use app_config::{AppConfig, CKBAppConfig, MinerAppConfig};
pub use args::{ExportArgs, ImportArgs, InitArgs, MinerArgs, RunArgs};
pub use exit_code::ExitCode;

use ckb_chain_spec::{consensus::Consensus, ChainSpec};
use ckb_instrument::Format;
use ckb_resource::ResourceLocator;
use ckb_verification::MerkleRootVerifier;
use clap::{value_t, ArgMatches};
use log::info;
use logger::LoggerInitGuard;
use std::error::Error;
use std::path::PathBuf;

pub struct Setup {
    subcommand_name: String,
    resource_locator: ResourceLocator,
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

        let resource_locator = locator_from_matches(matches)?;
        let config = AppConfig::load_for_subcommand(&resource_locator, subcommand_name)?;
        if config.is_bundled() {
            eprintln!("Not a CKB directory, initialize one with `ckb init`.");
            return Err(ExitCode::Config);
        }

        let is_sentry_enabled = is_daemon(&subcommand_name) && config.sentry().is_enabled();

        Ok(Setup {
            subcommand_name: subcommand_name.to_string(),
            resource_locator,
            config,
            is_sentry_enabled,
        })
    }

    pub fn setup_app(&self) -> Result<SetupGuard, ExitCode> {
        let logger_guard = logger::init(self.config.logger().clone())?;

        let sentry_guard = if self.is_sentry_enabled {
            let sentry_config = self.config.sentry();

            info!(target: "sentry", "**Notice**: \
                The ckb process will send stack trace to sentry on Rust panics. \
                This is enabled by default before mainnet, which can be opted out by setting \
                the option `dsn` to empty in the config file. The DSN is now {}", sentry_config.dsn);

            let guard = sentry_config.init();

            sentry::configure_scope(|scope| {
                scope.set_tag("subcommand", &self.subcommand_name);
            });

            Some(guard)
        } else {
            info!(target: "sentry", "sentry is disabled");
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

    pub fn import<'m>(self, matches: &ArgMatches<'m>) -> Result<ImportArgs, ExitCode> {
        let consensus = self.consensus()?;
        let config = self.config.into_ckb()?;
        let format = value_t!(matches.value_of(cli::ARG_FORMAT), Format)?;
        let source = value_t!(matches.value_of(cli::ARG_SOURCE), PathBuf)?;

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
        let format = value_t!(matches.value_of(cli::ARG_FORMAT), Format)?;
        let target = value_t!(matches.value_of(cli::ARG_TARGET), PathBuf)?;

        Ok(ExportArgs {
            config,
            consensus,
            format,
            target,
        })
    }

    pub fn init<'m>(matches: &ArgMatches<'m>) -> Result<InitArgs, ExitCode> {
        let locator = locator_from_matches(matches)?;
        let export_specs = matches.is_present(cli::ARG_EXPORT_SPECS);
        let list_specs = matches.is_present(cli::ARG_LIST_SPECS);
        let force = matches.is_present(cli::ARG_FORCE);
        let spec = matches.value_of(cli::ARG_SPEC).unwrap().to_string();
        let rpc_port = matches.value_of(cli::ARG_RPC_PORT).unwrap().to_string();
        let p2p_port = matches.value_of(cli::ARG_P2P_PORT).unwrap().to_string();
        let (log_to_file, log_to_stdout) = match matches.value_of(cli::ARG_LOG_TO) {
            Some("file") => (true, false),
            Some("stdout") => (false, true),
            Some("both") => (true, true),
            _ => unreachable!(),
        };

        Ok(InitArgs {
            locator,
            spec,
            rpc_port,
            p2p_port,
            export_specs,
            list_specs,
            force,
            log_to_file,
            log_to_stdout,
        })
    }

    fn chain_spec(&self) -> Result<ChainSpec, ExitCode> {
        let result = self.config.chain_spec(&self.resource_locator);
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
    spec.to_consensus().and_then(verify_genesis).map_err(|err| {
        eprintln!("{:?}", err);
        ExitCode::Config
    })
}

fn verify_genesis(consensus: Consensus) -> Result<Consensus, Box<Error>> {
    MerkleRootVerifier::new()
        .verify(consensus.genesis_block())
        .map_err(Box::new)?;
    Ok(consensus)
}

fn locator_from_matches<'m>(matches: &ArgMatches<'m>) -> Result<ResourceLocator, ExitCode> {
    let config_dir = match matches.value_of(cli::ARG_CONFIG_DIR) {
        Some(arg_config_dir) => PathBuf::from(arg_config_dir),
        None => ::std::env::current_dir()?,
    };
    ResourceLocator::with_root_dir(config_dir).map_err(Into::into)
}
