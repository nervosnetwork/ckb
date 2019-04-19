use ckb_test::specs::*;
use logger::{self, Config};
use std::env;

fn main() {
    let log_config = Config {
        filter: Some("info".to_owned()),
        ..Default::default()
    };
    let _logger_guard = logger::init(log_config).expect("init Logger");

    let binary = env::args()
        .nth(1)
        .unwrap_or_else(|| "../target/release/ckb".to_string());
    let start_port = env::args()
        .nth(2)
        .unwrap_or_else(|| "9000".to_string())
        .parse()
        .expect("invalid port number");
    if let Some(spec_name) = env::args().nth(3) {
        let spec: Box<Spec> = match &spec_name[..] {
            "block_relay_basic" => Box::new(BlockRelayBasic),
            "block_sync_basic" => Box::new(BlockSyncBasic),
            "mining_basic" => Box::new(MiningBasic),
            "pool_reconcile" => Box::new(PoolReconcile),
            "pool_trace" => Box::new(PoolTrace),
            "transaction_relay_basic" => Box::new(TransactionRelayBasic),
            "discovery" => Box::new(Discovery),
            "disconnect" => Box::new(Disconnect),
            "malformed_message" => Box::new(MalformedMessage),
            "depent_tx_in_same_block" => Box::new(DepentTxInSameBlock),
            "cellbase_immature_tx" => Box::new(CellbaseImmatureTx),
            _ => panic!("invalid spec"),
        };
        let net = spec.setup_net(&binary, start_port);
        spec.run(net);
    } else {
        let specs: Vec<Box<Spec>> = vec![
            Box::new(BlockRelayBasic),
            Box::new(BlockSyncBasic),
            Box::new(MiningBasic),
            Box::new(PoolReconcile),
            Box::new(PoolTrace),
            Box::new(TransactionRelayBasic),
            Box::new(Discovery),
            Box::new(Disconnect),
            Box::new(MalformedMessage),
            Box::new(DepentTxInSameBlock),
            Box::new(CellbaseImmatureTx),
        ];

        specs.iter().for_each(|spec| {
            let net = spec.setup_net(&binary, start_port);
            spec.run(net);
        })
    }
}
