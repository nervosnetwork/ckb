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
            ("type_hash", _) => cli::type_hash(&setup(&matches)),
            ("keygen", _) => cli::keygen(),
            _ => unreachable!(),
        },
        ("run", Some(_)) => {
            cli::run(setup(&matches));
        }
        ("miner", Some(_)) => cli::miner(&matches),
        ("export", Some(export_matches)) => cli::export(&setup(&matches), export_matches),
        ("import", Some(import_matches)) => cli::import(&setup(&matches), import_matches),
        _ => unreachable!(),
    }

    logger::flush();
}

fn setup(matches: &ArgMatches<'static>) -> Setup {
    let config_path = get_config_path(matches);
    let setup = match Setup::setup(&config_path) {
        Ok(setup) => {
            logger::init(setup.configs.logger.clone()).expect("Init Logger");
            setup
        }
        Err(e) => {
            eprintln!(
                "Failed to setup with config {}, cause err: {:?}",
                config_path.display(),
                e
            );
            ::std::process::exit(1);
        }
    };
    info!(target: "main", "Setup with config {}", config_path.display());
    setup
}
