mod helper;
mod subcommand;

use build_info::{get_version, Version};
use ckb_app_config::{cli, ExitCode, Setup};

fn run_app() -> Result<(), ExitCode> {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let version = get_version!();
    let app_matches = cli::get_matches(&version);
    match app_matches.subcommand() {
        (cli::CMD_INIT, Some(matches)) => return subcommand::init(Setup::init(&matches)?),
        (cli::CMD_CLI, Some(matches)) => {
            return match matches.subcommand() {
                (cli::CMD_KEYGEN, _) => subcommand::cli::keygen(),
                (cli::CMD_HASHES, Some(sub_matches)) => {
                    subcommand::cli::hashes(Setup::locator_from_matches(&matches)?, sub_matches)
                }
                _ => unreachable!(),
            };
        }
        _ => {
            // continue
        }
    }

    let setup = Setup::from_matches(&app_matches)?;
    let _guard = setup.setup_app(&version);

    match app_matches.subcommand() {
        (cli::CMD_RUN, _) => subcommand::run(setup.run()?, version),
        (cli::CMD_MINER, _) => subcommand::miner(setup.miner()?),
        (cli::CMD_PROF, Some(matches)) => subcommand::profile(setup.prof(&matches)?),
        (cli::CMD_EXPORT, Some(matches)) => subcommand::export(setup.export(&matches)?),
        (cli::CMD_IMPORT, Some(matches)) => subcommand::import(setup.import(&matches)?),
        _ => unreachable!(),
    }
}

fn main() {
    if let Some(exit_code) = run_app().err() {
        ::std::process::exit(exit_code.into());
    }
}
