#[macro_use]
extern crate clap;
extern crate ctrlc;
extern crate dir;
extern crate ethash;
#[macro_use]
extern crate log;
extern crate bigint;
extern crate logger;
extern crate nervos_chain as chain;
extern crate nervos_db as db;
extern crate nervos_miner as miner;
extern crate nervos_network as network;
extern crate nervos_notify;
extern crate nervos_pool as pool;
extern crate nervos_rpc as rpc;
extern crate nervos_sync as sync;
extern crate nervos_util as util;
extern crate nervos_verification;
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
