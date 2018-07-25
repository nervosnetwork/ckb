#[macro_use]
extern crate clap;
extern crate ctrlc;
extern crate dir;
extern crate ethash;
#[macro_use]
extern crate log;
extern crate bigint;
extern crate ckb_chain as chain;
extern crate ckb_db as db;
extern crate ckb_miner as miner;
extern crate ckb_network as network;
extern crate ckb_notify;
extern crate ckb_pool as pool;
extern crate ckb_rpc as rpc;
extern crate ckb_sync as sync;
extern crate ckb_util as util;
extern crate ckb_verification;
extern crate logger;
#[macro_use]
extern crate serde_derive;
extern crate config as config_tool;
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

    match Setup::new(&matches) {
        Ok(setup) => match matches.subcommand() {
            ("run", Some(_run_cmd)) => {
                cli::run(setup);
            }
            _ => {
                cli::run(setup);
            }
        },
        Err(e) => println!("Failed to setup, cause err {}", e.description()),
    }
}
