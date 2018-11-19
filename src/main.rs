#![feature(box_syntax)]
extern crate bigint;
extern crate bls;
#[macro_use]
extern crate clap;
extern crate crypto;
extern crate ctrlc;
extern crate dir;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
extern crate logger;
extern crate nervos_chain as chain;
extern crate nervos_core as core;
extern crate nervos_db as db;
extern crate nervos_miner as miner;
extern crate nervos_network as network;
extern crate nervos_pool as pool;
extern crate nervos_rpc as rpc;
extern crate nervos_time as time;
extern crate nervos_util as util;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate tera;
extern crate toml;

mod adapter;
mod cli;
mod config;

use config::Config;

fn main() {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let yaml = load_yaml!("cli/app.yml");

    let matches = clap::App::from_yaml(yaml).get_matches();

    let config = Config::parse(&matches);

    match matches.subcommand() {
        ("run", Some(_run_cmd)) => {
            cli::run(config);
        }
        ("signer", Some(signer_matches)) => cli::signer_cmd(signer_matches),
        _ => {
            cli::run(config);
        }
    }
}
