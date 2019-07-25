use ckb_test::specs::*;
use ckb_test::Spec;
use clap::{value_t, App, Arg};
use log::{error, info};
use rand::{seq::SliceRandom, thread_rng};
use std::any::Any;
use std::collections::HashMap;
use std::env;
use std::panic;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let _ = {
        let filter = ::std::env::var("CKB_LOG").unwrap_or_else(|_| "info".to_string());
        env_logger::builder().parse_filters(&filter).try_init()
    };

    let clap_app = clap_app();
    let matches = clap_app.get_matches();

    let binary = matches.value_of("binary").unwrap();
    let start_port = value_t!(matches, "port", u16).unwrap_or_else(|err| err.exit());
    let spec_names_to_run: Vec<_> = matches.values_of("specs").unwrap_or_default().collect();
    let max_time = if matches.is_present("max-time") {
        Some(value_t!(matches, "max-time", u64).unwrap_or_else(|err| err.exit()))
    } else {
        None
    };

    let all_specs = build_specs();

    if matches.is_present("list-specs") {
        let mut names: Vec<_> = all_specs.keys().collect();
        names.sort();
        for spec_name in names {
            println!("{}", spec_name);
        }
        return;
    }

    let specs = filter_specs(all_specs, spec_names_to_run);

    info!("binary: {}", binary);
    info!("start port: {}", start_port);
    info!("max time: {:?}", max_time);

    let total = specs.len();
    let start_time = Instant::now();
    let mut specs_iter = specs.into_iter().enumerate();
    let mut rerun_specs = vec![];
    let mut panic_error: Option<Box<dyn Any + Send>> = None;

    for (index, (spec_name, spec)) in &mut specs_iter {
        info!(
            "{}/{} .............. Running {}",
            index + 1,
            total,
            spec_name
        );
        let now = Instant::now();
        let result = panic::catch_unwind(panic::AssertUnwindSafe(move || {
            let net = spec.setup_net(&binary, start_port);
            spec.run(net);
        }));
        info!(
            "{}/{} -------------> Completed {} in {} seconds",
            index + 1,
            total,
            spec_name,
            now.elapsed().as_secs()
        );

        panic_error = result.err();
        if panic_error.is_some() {
            rerun_specs.push(spec_name);
            break;
        }
        if start_time.elapsed().as_secs() > max_time.unwrap_or_else(u64::max_value) {
            error!(
                "Exit ckb-test, because total running time exeedes {} seconds",
                max_time.unwrap_or_default()
            );
            break;
        }
    }

    rerun_specs.extend(specs_iter.map(|t| (t.1).0));
    if rerun_specs.is_empty() {
        return;
    }

    if panic_error.is_some() {
        error!("ckb-failed on spec {}", rerun_specs[0]);
        info!("You can rerun remaining specs using following command:");
    } else {
        info!("You can run the skipped specs using following command:");
    }

    info!(
        "{} --bin {} --port {} {}",
        canonicalize_path(env::args().nth(0).unwrap_or_else(|| "ckb-test".to_string())).display(),
        canonicalize_path(binary).display(),
        start_port,
        rerun_specs.join(" "),
    );

    if let Some(err) = panic_error {
        panic::resume_unwind(err);
    }
}

type SpecMap = HashMap<&'static str, Box<dyn Spec>>;
type SpecTuple<'a> = (&'a str, Box<dyn Spec>);

fn clap_app() -> App<'static, 'static> {
    App::new("ckb-test")
        .arg(
            Arg::with_name("binary")
                .short("b")
                .long("bin")
                .takes_value(true)
                .value_name("PATH")
                .help("Path to ckb executable")
                .default_value("../target/release/ckb"),
        )
        .arg(
            Arg::with_name("port")
                .short("p")
                .long("port")
                .takes_value(true)
                .help("Starting port number used to start ckb nodes")
                .default_value("9000"),
        )
        .arg(
            Arg::with_name("max-time")
                .long("max-time")
                .takes_value(true)
                .value_name("SECONDS")
                .help("Exit when total running time exceeds this limit"),
        )
        .arg(Arg::with_name("list-specs").long("list-specs"))
        .arg(Arg::with_name("specs").multiple(true))
}

fn filter_specs(mut all_specs: SpecMap, spec_names_to_run: Vec<&str>) -> Vec<SpecTuple> {
    if spec_names_to_run.is_empty() {
        let mut specs: Vec<_> = all_specs.into_iter().collect();
        specs.shuffle(&mut thread_rng());
        specs
    } else {
        let mut specs = Vec::with_capacity(spec_names_to_run.len());
        for spec_name in spec_names_to_run {
            specs.push((
                spec_name,
                all_specs.remove(spec_name).unwrap_or_else(|| {
                    eprintln!("Unknown spec {}", spec_name);
                    std::process::exit(1);
                }),
            ));
        }
        specs
    }
}

fn canonicalize_path<P: AsRef<Path>>(path: P) -> PathBuf {
    path.as_ref()
        .canonicalize()
        .unwrap_or_else(|_| path.as_ref().to_path_buf())
}

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
    specs.insert(
        "chain_contains_invalid_block",
        Box::new(ChainContainsInvalidBlock),
    );
    specs.insert(
        "fork_contains_invalid_block",
        Box::new(ForkContainsInvalidBlock),
    );
    specs.insert("chain_fork_1", Box::new(ChainFork1));
    specs.insert("chain_fork_2", Box::new(ChainFork2));
    specs.insert("chain_fork_3", Box::new(ChainFork3));
    specs.insert("chain_fork_4", Box::new(ChainFork4));
    specs.insert("chain_fork_5", Box::new(ChainFork5));
    specs.insert("chain_fork_6", Box::new(ChainFork6));
    specs.insert("chain_fork_7", Box::new(ChainFork7));
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
        "compact_block_missing_fresh_txs",
        Box::new(CompactBlockMissingFreshTxs),
    );
    specs.insert(
        "compact_block_missing_not_fresh_txs",
        Box::new(CompactBlockMissingNotFreshTxs),
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
    specs.insert("ibd_process", Box::new(IBDProcess));

    specs
}
