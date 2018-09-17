extern crate ckb_network as network;
#[macro_use]
extern crate log;
extern crate env_logger;
#[macro_use]
extern crate crossbeam_channel;
extern crate clap;
extern crate parking_lot;
extern crate tempdir;

mod cases;

use std::env;

fn main() {
    let log_level = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    env::set_var("RUST_LOG", log_level);
    env_logger::init();

    let arg_boot_nodes = clap::Arg::with_name("boot-nodes")
        .long("boot-nodes")
        .takes_value(true)
        .help("Boot node number");
    let arg_other_nodes = clap::Arg::with_name("other-nodes")
        .long("other-nodes")
        .takes_value(true)
        .help("Other node number");
    let arg_init_wait = clap::Arg::with_name("init-wait")
        .long("init-wait")
        .takes_value(true)
        .help("Wait time(ms) after one node's network started");
    let arg_keep_connect = clap::Arg::with_name("keep-connect")
        .long("keep-connect")
        .takes_value(true)
        .help("Keep connect for how many seconds");

    let matches = clap::App::new("test")
        .subcommand(clap::SubCommand::with_name("simple"))
        .subcommand(
            clap::SubCommand::with_name("many_nodes")
                .arg(arg_boot_nodes.clone())
                .arg(arg_other_nodes.clone())
                .arg(arg_init_wait.clone())
                .arg(
                    clap::Arg::with_name("should-connect")
                        .long("should-connect")
                        .takes_value(true)
                        .help("Should connect in how many seconds"),
                ).arg(arg_keep_connect.clone()),
        ).subcommand(
            clap::SubCommand::with_name("many_messages")
                .arg(arg_boot_nodes)
                .arg(arg_other_nodes)
                .arg(arg_init_wait)
                .arg(
                    clap::Arg::with_name("timer")
                        .long("timer")
                        .takes_value(true)
                        .help("Send message every <timer>(ms)"),
                ).arg(
                    clap::Arg::with_name("send-msgs")
                        .long("send-msgs")
                        .takes_value(true)
                        .help("How many messages to send every timeout"),
                ).arg(arg_keep_connect),
        ).get_matches();

    match matches.subcommand() {
        ("simple", Some(m)) => cases::simple::test::run(m),
        ("many_nodes", Some(m)) => cases::many_nodes::test::run(m),
        ("many_messages", Some(m)) => cases::many_messages::test::run(m),
        _ => panic!("Unexpected arguments: {:?}", matches),
    }
}
