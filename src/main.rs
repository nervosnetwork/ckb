#[macro_use]
extern crate clap;
extern crate ctrlc;
extern crate dir;
#[macro_use]
extern crate log;
extern crate bigint;
extern crate ckb_chain as chain;
extern crate ckb_core as core;
extern crate ckb_db as db;
extern crate ckb_miner as miner;
extern crate ckb_network as network;
extern crate ckb_notify;
extern crate ckb_pool as pool;
extern crate ckb_rpc as rpc;
extern crate ckb_sync as sync;
extern crate ckb_util as util;
extern crate logger;
#[macro_use]
extern crate serde_derive;
extern crate ckb_script as script;
extern crate config as config_tool;
extern crate crypto;
extern crate reqwest;
extern crate serde_json;
extern crate serde_yaml;
#[cfg(test)]
extern crate tempdir;

mod chain_spec;
mod cli;
mod helper;
mod setup;

use setup::Setup;

fn main() {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let yaml = load_yaml!("cli/app.yml");
    let matches = clap::App::from_yaml(yaml).get_matches();

    match matches.subcommand() {
        ("cli", Some(client_matches)) => match client_matches.subcommand() {
            ("rpc", Some(rpc_matches)) => cli::rpc(rpc_matches),
            ("sign", Some(sign_matches)) => cli::sign(sign_matches),
            ("keygen", _) => cli::keygen(),
            _ => println!("Invalid client subcommand"),
        },
        ("run", Some(run_matches)) => match Setup::new(&run_matches) {
            Ok(setup) => cli::run(setup),
            Err(e) => println!("Failed to setup, cause err {}", e.description()),
        },
        _ => println!("Invalid subcommand"),
    }
}
