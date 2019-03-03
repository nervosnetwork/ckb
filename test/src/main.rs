use ckb_test::specs::*;
use logger::{self, Config};
use std::env;

fn main() {
    let log_config = Config {
        filter: Some("info".to_owned()),
        color: true,
        file: None,
    };
    logger::init(log_config).expect("init Logger");

    let binary = env::args()
        .nth(1)
        .unwrap_or_else(|| "../target/release/ckb".to_string());
    let start_port = env::args()
        .nth(2)
        .unwrap_or_else(|| "9000".to_string())
        .parse()
        .expect("invalid port number");

    let specs: Vec<Box<Spec>> = vec![
        Box::new(BlockRelayBasic {}),
        Box::new(TransactionRelayBasic {}),
        Box::new(BlockSyncBasic {}),
        Box::new(MiningBasic {}),
    ];

    specs.iter().for_each(|spec| {
        let net = spec.setup_net(&binary, start_port);
        spec.run(&net);
    })
}
