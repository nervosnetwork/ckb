use ckb_logger::{self, Config};
use ckb_test::specs::*;
use ckb_test::Spec;
use clap::{value_t_or_exit, App, Arg};
use log::info;
use std::collections::HashMap;
use std::mem;

fn main() {
    let log_config = Config {
        filter: Some("info".to_owned()),
        ..Default::default()
    };
    let _logger_guard = ckb_logger::init(log_config).expect("init Logger");

    let clap_app = App::new("ckb-test")
        .arg(
            Arg::with_name("binary")
                .short("b")
                .long("bin")
                .required(true)
                .takes_value(true)
                .value_name("PATH")
                .help("Path to ckb executable")
                .default_value("../target/release/ckb"),
        )
        .arg(
            Arg::with_name("port")
                .short("p")
                .long("port")
                .required(true)
                .takes_value(true)
                .help("Starting port number used to start ckb nodes")
                .default_value("9000"),
        )
        .arg(Arg::with_name("specs").multiple(true));
    let matches = clap_app.get_matches();

    let binary = matches.value_of("binary").unwrap();
    let start_port = value_t_or_exit!(matches, "port", u16);
    let spec_names_to_run: Vec<_> = matches.values_of("specs").unwrap_or_default().collect();

    let mut specs = build_specs();
    if !spec_names_to_run.is_empty() {
        let mut remaining_specs = mem::replace(&mut specs, HashMap::new());
        for spec_name in spec_names_to_run {
            specs.insert(
                spec_name,
                remaining_specs
                    .remove(spec_name)
                    .expect(&format!("expect spec {}", spec_name)),
            );
        }
    }

    info!("binary: {}", binary);
    info!("start port: {}", start_port);

    for (spec_name, spec) in specs {
        info!("Running {}", spec_name);
        let net = spec.setup_net(&binary, start_port);
        spec.run(net);
    }
}

type SpecMap = HashMap<&'static str, Box<dyn Spec>>;

fn build_specs() -> SpecMap {
    let mut specs = SpecMap::new();

    specs.insert("block_relay_basic", Box::new(BlockRelayBasic));
    specs.insert("block_sync_from_one", Box::new(BlockSyncFromOne));
    specs.insert("block_sync_forks", Box::new(BlockSyncForks));
    specs.insert(
        "block_sync_duplicated_and_reconnect",
        Box::new(BlockSyncDuplicatedAndReconnect),
    );
    specs.insert("block_sync_orphan_blocks", Box::new(BlockSyncOrphanBlocks));
    specs.insert("sync_timeout", Box::new(SyncTimeout));
    specs.insert("chain_fork_1", Box::new(ChainFork1));
    specs.insert("chain_fork_2", Box::new(ChainFork2));
    // FIXME these 4 tests will fail, because of https://github.com/nervosnetwork/ckb/pull/1164
    // node will be banned, we need to add `listbanned` rpc and modify test code to assert that node has been banned
    // https://bitcoincore.org/en/doc/0.16.0/rpc/network/listbanned/
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
    // FIXME: There is a probability of failure on low resouce CI server
    // specs.insert(
    //     "transaction_relay_multiple",
    //     Box::new(TransactionRelayMultiple),
    // );
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
    specs.insert("compact_block_empty", Box::new(CompactBlockEmpty));
    specs.insert(
        "compact_block_empty_parent_unknown",
        Box::new(CompactBlockEmptyParentUnknown),
    );
    specs.insert("compact_block_prefilled", Box::new(CompactBlockPrefilled));
    specs.insert(
        "compact_block_missing_txs",
        Box::new(CompactBlockMissingTxs),
    );
    specs.insert(
        "compact_block_lose_get_block_transactions",
        Box::new(CompactBlockLoseGetBlockTransactions),
    );
    specs.insert(
        "compact_block_relay_parent_of_orphan_block",
        Box::new(CompactBlockRelayParentOfOrphanBlock),
    );
    specs.insert(
        "compact_block_relay_less_then_shared_best_known",
        Box::new(CompactBlockRelayLessThenSharedBestKnown),
    );
    specs.insert("invalid_locator_size", Box::new(InvalidLocatorSize));
    specs.insert("tx_pool_size_limit", Box::new(SizeLimit));
    specs.insert("tx_pool_cycles_limit", Box::new(CyclesLimit));
    specs.insert("alert_propagation", Box::new(AlertPropagation::default()));
    specs.insert("indexer_basic", Box::new(IndexerBasic));
    specs.insert("genesis_issued_cells", Box::new(GenesisIssuedCells));

    specs
}
