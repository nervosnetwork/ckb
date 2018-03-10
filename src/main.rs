extern crate bigint;
#[macro_use]
extern crate clap;
extern crate ctrlc;
#[macro_use]
extern crate log;
extern crate logger;
extern crate nervos_chain as chain;
extern crate nervos_core as core;
extern crate nervos_db as db;
extern crate nervos_miner as miner;
extern crate nervos_network as network;
extern crate nervos_pool as pool;
extern crate nervos_time as time;
extern crate nervos_util as util;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate toml;

mod config;
mod adapter;
mod cli;

fn main() {
    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "1");

    let matches = clap_app!(nervos =>
        (version: "0.1")
        (author: "Nervos <dev@nervos.org>")
        (about: "Nervos")
        (@subcommand run =>
            (about: "run nervos")
            (@arg config: -c --config +takes_value "Sets a custom config file")
        )
        (@subcommand new =>
            (about: "new nervos config")
        )
    ).get_matches();

    match matches.subcommand() {
        ("run", Some(run_cmd)) => {
            let config_path = run_cmd.value_of("config").unwrap_or("default.toml");
            cli::run(config_path);
        }
        ("new", Some(_new_cmd)) => {}
        _ => {}
    }
}
