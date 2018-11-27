#[macro_use]
extern crate clap;
extern crate ctrlc;
extern crate dir;
#[macro_use]
extern crate log;
extern crate bigint;
extern crate ckb_chain;
extern crate ckb_chain_spec;
extern crate ckb_core;
extern crate ckb_db;
extern crate ckb_miner;
extern crate ckb_network;
extern crate ckb_notify;
extern crate ckb_pool;
extern crate ckb_rpc;
extern crate ckb_shared;
extern crate ckb_sync;
extern crate ckb_util;
extern crate hash;
extern crate logger;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate build_info;
extern crate ckb_instrument;
extern crate ckb_pow;
extern crate config as config_tool;
extern crate crypto;
extern crate faster_hex;
extern crate serde_json;
#[cfg(test)]
extern crate tempfile;

mod cli;
mod helper;
mod setup;

use build_info::Version;
use setup::{get_config_path, Setup};

fn main() {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let yaml = load_yaml!("cli/app.yml");
    let version = get_version!();
    let matches = clap::App::from_yaml(yaml)
        .version(version.short().as_str())
        .long_version(version.long().as_str())
        .get_matches();

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
            ("sign", Some(sign_matches)) => cli::sign(&setup, sign_matches),
            ("type_hash", Some(type_hash_matches)) => cli::type_hash(&setup, type_hash_matches),
            ("keygen", _) => cli::keygen(),
            _ => unreachable!(),
        },
        ("run", Some(_)) => {
            info!(target: "main", "Start with config {}", config_path.display());
            cli::run(setup);
        }
        ("export", Some(export_matches)) => cli::export(&setup, export_matches),
        ("import", Some(import_matches)) => cli::import(&setup, import_matches),
        _ => unreachable!(),
    }

    logger::flush();
}
