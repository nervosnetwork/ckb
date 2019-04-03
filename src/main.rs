mod cli;
mod helper;
mod setup;

use crate::setup::{get_config_path, Setup};
use clap::ArgMatches;
use log::info;

fn main() {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let matches = cli::get_matches();

    match matches.subcommand() {
        ("cli", Some(cli_matches)) => match cli_matches.subcommand() {
            ("keygen", _) => cli::keygen(),
            _ => unreachable!(),
        },
        ("run", Some(run_matches)) => {
            let setup = setup(&run_matches);
            let _logger_guard = logger::init(setup.configs.logger.clone()).expect("Init Logger");
            let _sentry_guard = setup.configs.sentry.clone().init();
            helper::deadlock_detection();
            cli::run(setup);
        }
        ("miner", Some(miner_matches)) => cli::miner(&miner_matches),
        ("export", Some(export_matches)) => cli::export(&setup(&export_matches), export_matches),
        ("import", Some(import_matches)) => cli::import(&setup(&import_matches), import_matches),
        _ => unreachable!(),
    }
}

fn setup(matches: &ArgMatches<'static>) -> Setup {
    let config_path = get_config_path(matches);
    info!(target: "main", "Setup with config {}", config_path.display());
    Setup::setup(&config_path).unwrap_or_else(|e| {
        eprintln!(
            "Failed to setup with config {}, cause err: {:?}",
            config_path.display(),
            e
        );
        ::std::process::exit(1);
    })
}
