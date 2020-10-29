//! TODO(doc): @doitian
mod helper;
mod setup_guard;
mod subcommand;

use ckb_app_config::{cli, ExitCode, Setup};
use ckb_build_info::Version;

use setup_guard::SetupGuard;

pub(crate) const LOG_TARGET_MAIN: &str = "main";
pub(crate) const LOG_TARGET_SENTRY: &str = "sentry";

/// TODO(doc): @doitian
pub fn run_app(version: Version) -> Result<(), ExitCode> {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let app_matches = cli::get_matches(&version);
    match app_matches.subcommand() {
        (cli::CMD_INIT, Some(matches)) => {
            return subcommand::init(Setup::init(&matches)?);
        }
        (cli::CMD_LIST_HASHES, Some(matches)) => {
            return subcommand::list_hashes(Setup::root_dir_from_matches(&matches)?, matches);
        }
        (cli::CMD_PEERID, Some(matches)) => match matches.subcommand() {
            (cli::CMD_GEN_SECRET, Some(matches)) => return Setup::gen(&matches),
            (cli::CMD_FROM_SECRET, Some(matches)) => {
                return subcommand::peer_id(Setup::peer_id(&matches)?);
            }
            _ => {}
        },
        _ => {
            // continue
        }
    }

    let setup = Setup::from_matches(&app_matches)?;
    let _guard = SetupGuard::from_setup(&setup, &version)?;

    match app_matches.subcommand() {
        (cli::CMD_RUN, Some(matches)) => subcommand::run(setup.run(&matches)?, version),
        (cli::CMD_MINER, Some(matches)) => subcommand::miner(setup.miner(&matches)?),
        (cli::CMD_REPLAY, Some(matches)) => subcommand::replay(setup.replay(&matches)?),
        (cli::CMD_EXPORT, Some(matches)) => subcommand::export(setup.export(&matches)?),
        (cli::CMD_IMPORT, Some(matches)) => subcommand::import(setup.import(&matches)?),
        (cli::CMD_STATS, Some(matches)) => subcommand::stats(setup.stats(&matches)?),
        (cli::CMD_RESET_DATA, Some(matches)) => subcommand::reset_data(setup.reset_data(&matches)?),
        (cli::CMD_MIGRATE, Some(matches)) => subcommand::migrate(setup.migrate(&matches)?),
        _ => unreachable!(),
    }
}
