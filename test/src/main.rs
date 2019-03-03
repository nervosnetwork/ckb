use ckb_test::specs::*;
use logger::{self, Config};

fn main() {
    let log_config = Config {
        filter: Some("info".to_owned()),
        color: true,
        file: None,
    };
    logger::init(log_config).expect("Init Logger");

    let binary = "../target/release/ckb";
    let start_port = 9000;

    let specs: Vec<Box<Spec>> = vec![
        Box::new(BlockRelayBasic {}),
        Box::new(TransactionRelayBasic {}),
        Box::new(BlockSyncBasic {}),
        Box::new(MiningBasic {}),
    ];

    specs.iter().for_each(|spec| {
        let net = spec.setup_net(binary, start_port);
        spec.run(&net);
    })
}
