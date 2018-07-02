#[macro_use]
extern crate clap;
extern crate ctrlc;
extern crate dir;
extern crate ethash;
#[macro_use]
extern crate log;
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
#[macro_use]
extern crate serde_derive;
extern crate config;

mod cli;
mod spec;

use spec::Spec;

fn main() {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let yaml = load_yaml!("cli/app.yml");
    let matches = clap::App::from_yaml(yaml).get_matches();
    let spec = Spec::new(&matches).unwrap();

    match matches.subcommand() {
        ("run", Some(_run_cmd)) => {
            cli::run(spec);
        }
        _ => {
            cli::run(spec);
        }
    }
}
