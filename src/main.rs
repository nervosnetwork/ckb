mod cli;
mod helper;
mod setup;

use crate::setup::{get_config_path, Setup};
use log::info;

fn main() {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let matches = cli::get_matches();
    let config_path = get_config_path(&matches);
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

    match matches.subcommand() {
        ("cli", Some(cli_matches)) => match cli_matches.subcommand() {
            ("type_hash", _) => cli::type_hash(&setup),
            ("keygen", _) => cli::keygen(),
            _ => unreachable!(),
        },
        ("run", Some(_)) => {
            info!(target: "main", "Start with config {}", config_path.display());
            cli::run(setup);
        }
        ("miner", Some(_)) => cli::miner(setup),
        ("export", Some(export_matches)) => cli::export(&setup, export_matches),
        ("import", Some(import_matches)) => cli::import(&setup, import_matches),
        _ => unreachable!(),
    }

    logger::flush();
}
