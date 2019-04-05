mod app_config;
mod args;
pub mod cli;
mod exit_code;
mod sentry_config;

pub use app_config::AppConfig;
pub use args::{ExportArgs, ImportArgs, InitArgs, MinerArgs, RunArgs};
pub use exit_code::ExitCode;

use ckb_chain_spec::{consensus::Consensus, ChainSpec};
use ckb_instrument::Format;
use ckb_resource::ResourceLocator;
use clap::{value_t, ArgMatches};
use logger::LoggerInitGuard;
use std::path::PathBuf;

pub struct Setup {
    subcommand_name: String,
    resource_locator: ResourceLocator,
    config: AppConfig,
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

        Ok(Setup {
            subcommand_name: subcommand_name.to_string(),
            resource_locator,
            config,
        })
    }

    pub fn setup_app(&self) -> Result<SetupGuard, ExitCode> {
        let logger_guard = logger::init(self.config.logger().clone())?;
        let sentry_guard = if is_daemon(&self.subcommand_name) {
            Some(self.config.sentry().init())
        } else {
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
        let spec = matches.value_of(cli::ARG_SPEC).unwrap().to_string();
        let rpc_port = matches.value_of(cli::ARG_RPC_PORT).unwrap().to_string();
        let p2p_port = matches.value_of(cli::ARG_P2P_PORT).unwrap().to_string();

        Ok(InitArgs {
            locator,
            spec,
            rpc_port,
            p2p_port,
            export_specs,
            list_specs,
        })
    }

    fn chain_spec(&self) -> Result<ChainSpec, ExitCode> {
        self.config.chain_spec(&self.resource_locator)
    }

    fn consensus(&self) -> Result<Consensus, ExitCode> {
        consensus_from_spec(&self.chain_spec()?)
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
    spec.to_consensus().map_err(|err| {
        eprintln!("{:?}", err);
        ExitCode::Config
    })
}

fn locator_from_matches<'m>(matches: &ArgMatches<'m>) -> Result<ResourceLocator, ExitCode> {
    let config_dir = match matches.value_of(cli::ARG_CONFIG_DIR) {
        Some(arg_config_dir) => PathBuf::from(arg_config_dir),
        None => ::std::env::current_dir()?,
    };
    ResourceLocator::with_root_dir(config_dir).map_err(Into::into)
}
