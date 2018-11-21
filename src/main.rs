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
use setup::Setup;

pub const DEFAULT_CONFIG_FILENAME: &str = "config.json";
pub const DEFAULT_CONFIG: &str = include_str!("config/default.json");

fn main() {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let yaml = load_yaml!("cli/app.yml");
    let version = get_version!();
    let matches = clap::App::from_yaml(yaml)
        .version(version.short().as_str())
        .long_version(version.long().as_str())
        .get_matches();

    match matches.subcommand() {
        ("cli", Some(client_matches)) => match client_matches.subcommand() {
            ("sign", Some(sign_matches)) => match Setup::new(&sign_matches) {
                Ok(setup) => cli::sign(&setup, sign_matches),
                Err(e) => println!("Failed to setup, cause err {}", e.description()),
            },
            ("type_hash", Some(matches)) => match Setup::new(&matches) {
                Ok(setup) => cli::type_hash(&setup, matches),
                Err(e) => println!("Failed to setup, cause err {}", e.description()),
            },
            ("keygen", _) => cli::keygen(),
            _ => unreachable!(),
        },
        ("run", Some(run_matches)) => match Setup::new(&run_matches) {
            Ok(setup) => cli::run(setup),
            Err(e) => println!("Failed to setup, cause err {:?}", e),
        },
        ("export", Some(export_matches)) => cli::export(&export_matches),
        ("import", Some(import_matches)) => cli::import(&import_matches),
        _ => unreachable!(),
    }
}
