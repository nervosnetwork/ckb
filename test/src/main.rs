use ckb_logger::{self, Config};
use ckb_test::specs::*;
use ckb_test::Spec;
use std::collections::HashMap;
use std::env;

fn main() {
    let log_config = Config {
        filter: Some("info".to_owned()),
        ..Default::default()
    };
    let _logger_guard = ckb_logger::init(log_config).expect("init Logger");

    let binary = env::args()
        .nth(1)
        .unwrap_or_else(|| "../target/release/ckb".to_string());
    let start_port = env::args()
        .nth(2)
        .unwrap_or_else(|| "9000".to_string())
        .parse()
        .expect("invalid port number");
    let mut specs: HashMap<&str, Box<dyn Spec>> = HashMap::new();
    specs.insert("block_relay_basic", Box::new(BlockRelayBasic));
    specs.insert("block_sync_basic", Box::new(BlockSyncBasic));
    specs.insert("sync_timeout", Box::new(SyncTimeout));
    specs.insert("chain_fork_1", Box::new(ChainFork1));
    specs.insert("chain_fork_2", Box::new(ChainFork2));
    // FIXME these 4 tests are failed on v0.15.0, because of https://github.com/nervosnetwork/ckb/pull/1164
    // node will be banned, we need to add `listbanned` rpc and modify test code to assert that node has been banned
    // https://bitcoincore.org/en/doc/0.16.0/rpc/network/listbanned/
    // plan to fix it on develop branch
    // specs.insert("chain_fork_3", Box::new(ChainFork3));
    // specs.insert("chain_fork_4", Box::new(ChainFork4));
    // specs.insert("chain_fork_5", Box::new(ChainFork5));
    // specs.insert("chain_fork_6", Box::new(ChainFork6));
    // specs.insert("chain_fork_7", Box::new(ChainFork7));
    specs.insert("mining_basic", Box::new(MiningBasic));
    specs.insert("mining_bootstrap_cellbase", Box::new(BootstrapCellbase));
    specs.insert("mining_template_size_limit", Box::new(TemplateSizeLimit));
    specs.insert("pool_reconcile", Box::new(PoolReconcile));
    specs.insert("pool_resurrect", Box::new(PoolResurrect));
    specs.insert("transaction_relay_basic", Box::new(TransactionRelayBasic));
    specs.insert(
        "transaction_relay_multiple",
        Box::new(TransactionRelayMultiple),
    );
    specs.insert("discovery", Box::new(Discovery));
    // TODO enable this after p2p lib resolve close timeout issue
    // specs.insert("disconnect", Box::new(Disconnect));
    specs.insert("malformed_message", Box::new(MalformedMessage));
    specs.insert("depent_tx_in_same_block", Box::new(DepentTxInSameBlock));
    // TODO enable these after proposed/pending pool tip verfiry logic changing
    // specs.insert("cellbase_maturity", Box::new(CellbaseMaturity));
    specs.insert("valid_since", Box::new(ValidSince));
    specs.insert(
        "different_txs_with_same_input",
        Box::new(DifferentTxsWithSameInput),
    );
    // FIXME this failed on v0.15.0, because of https://github.com/nervosnetwork/ckb/pull/1169
    // plan to fix it on develop branch after https://github.com/nervosnetwork/ckb/pull/1141
    // specs.insert("compact_block_basic", Box::new(CompactBlockBasic));
    specs.insert("invalid_locator_size", Box::new(InvalidLocatorSize));
    specs.insert("tx_pool_size_limit", Box::new(SizeLimit));
    specs.insert("tx_pool_cycles_limit", Box::new(CyclesLimit));
    specs.insert("alert_propagation", Box::new(AlertPropagation::default()));
    specs.insert("indexer_basic", Box::new(IndexerBasic));
    specs.insert("genesis_issued_cells", Box::new(GenesisIssuedCells));

    if let Some(spec_name) = env::args().nth(3) {
        if let Some(spec) = specs.get(spec_name.as_str()) {
            let net = spec.setup_net(&binary, start_port);
            spec.run(net);
        }
    } else {
        specs.values().for_each(|spec| {
            let net = spec.setup_net(&binary, start_port);
            spec.run(net);
        })
    }
}
