mod helper;
mod subcommand;

use ckb_app_config::{cli, ExitCode, Setup};

fn run_app() -> Result<(), ExitCode> {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let app_matches = cli::get_matches();
    match app_matches.subcommand() {
        (cli::CMD_INIT, Some(matches)) => return subcommand::init(Setup::init(&matches)?),
        (cli::CMD_CLI, Some(matches)) => {
            return match matches.subcommand() {
                (cli::CMD_KEYGEN, _) => subcommand::cli::keygen(),
                _ => unreachable!(),
            };
        }
        _ => {
            // continue
        }
    }

    let setup = Setup::from_matches(&app_matches)?;
    let _guard = setup.setup_app();

    match app_matches.subcommand() {
        (cli::CMD_RUN, _) => subcommand::run(setup.run()?),
        (cli::CMD_MINER, _) => subcommand::miner(setup.miner()?),
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
