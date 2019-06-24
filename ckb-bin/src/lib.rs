mod args;
mod cli_cmds;
mod helper;
mod setup;
mod subcommand;
mod utils;

use ckb_app_config::ExitCode;
use ckb_build_info::Version;
use cli_cmds::get_matches;
use setup::Setup;
use subcommand::cli::cli_main;

pub(crate) const LOG_TARGET_MAIN: &str = "main";
pub(crate) const LOG_TARGET_SENTRY: &str = "sentry";

pub fn run_app(version: Version) -> Result<(), ExitCode> {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let app_matches = get_matches(&version);
    match app_matches.subcommand() {
        (cli_cmds::CMD_INIT, Some(matches)) => return subcommand::init(Setup::init(&matches)?),
        (cli_cmds::CMD_CLI, Some(matches)) => {
            return match matches.subcommand() {
                (cli_cmds::CMD_BLAKE160, Some(sub_matches)) => {
                    subcommand::cli::blake160(sub_matches)
                }
                (cli_cmds::CMD_BLAKE256, Some(sub_matches)) => {
                    subcommand::cli::blake256(sub_matches)
                }
                (cli_cmds::CMD_SECP256K1_LOCK, Some(sub_matches)) => {
                    subcommand::cli::secp256k1_lock(sub_matches)
                }
                (cli_cmds::CMD_HASHES, Some(sub_matches)) => {
                    subcommand::cli::hashes(Setup::root_dir_from_matches(&matches)?, sub_matches)
                }
                _ => cli_main::cli_main(version, &matches).map_err(|_| ExitCode::Cli),
            };
        }
        _ => {
            // continue
        }
    }

    let setup = Setup::from_matches(&app_matches)?;
    let _guard = setup.setup_app(&version);

    match app_matches.subcommand() {
        (cli_cmds::CMD_RUN, _) => subcommand::run(setup.run()?, version),
        (cli_cmds::CMD_MINER, _) => subcommand::miner(setup.miner()?),
        (cli_cmds::CMD_PROF, Some(matches)) => subcommand::profile(setup.prof(&matches)?),
        (cli_cmds::CMD_EXPORT, Some(matches)) => subcommand::export(setup.export(&matches)?),
        (cli_cmds::CMD_IMPORT, Some(matches)) => subcommand::import(setup.import(&matches)?),
        (cli_cmds::CMD_STATS, Some(matches)) => subcommand::stats(setup.stats(&matches)?),
        _ => unreachable!(),
    }
}
