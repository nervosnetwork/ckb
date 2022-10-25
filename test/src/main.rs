use ckb_channel::unbounded;
use ckb_logger::{error, info, warn};
use ckb_test::specs::*;
use ckb_test::{
    global::{self, BINARY_PATH, PORT_COUNTER, VENDOR_PATH},
    worker::{Notify, Workers},
    Spec,
};
use ckb_types::core::ScriptHashType;
use ckb_util::Mutex;
use clap::{App, Arg};
use rand::{seq::SliceRandom, thread_rng};
use std::any::Any;
use std::cmp::min;
use std::env;
use std::fs::{self, read_to_string, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
enum TestResultStatus {
    Passed,
    Failed,
    Panicked,
}

struct TestResult {
    spec_name: String,
    status: TestResultStatus,
    duration: u64,
}

#[allow(clippy::cognitive_complexity)]
fn main() {
    env::set_var("RUST_BACKTRACE", "full");

    let clap_app = clap_app();
    let matches = clap_app.get_matches();

    let binary = matches.value_of_t_or_exit::<PathBuf>("binary");
    let start_port = matches.value_of_t_or_exit::<u16>("port");
    let spec_names_to_run: Vec<_> = matches.values_of("specs").unwrap_or_default().collect();
    let max_time = if matches.is_present("max-time") {
        matches.value_of_t_or_exit::<u64>("max-time")
    } else {
        0
    };
    let worker_count = matches.value_of_t_or_exit::<usize>("concurrent");
    let vendor = matches
        .value_of_t::<PathBuf>("vendor")
        .unwrap_or_else(|_| current_dir().join("vendor"));
    let log_file_opt = matches
        .value_of("log-file")
        .map(PathBuf::from_str)
        .transpose()
        .unwrap_or_else(|err| panic!("failed to parse the log file path since {}", err));
    let fail_fast = !matches.is_present("no-fail-fast");
    let report = !matches.is_present("no-report");
    let clean_tmp = !matches.is_present("keep-tmp-data");
    let verbose = matches.is_present("verbose");

    let logger_guard = {
        let filter = if !verbose {
            env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string())
        } else {
            format!(
                "{},{}=trace",
                env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
                module_path!(),
            )
        };
        let mut logger_config = ckb_logger_config::Config {
            filter: Some(filter),
            ..Default::default()
        };
        if let Some(log_file) = log_file_opt {
            let full_log_file = if log_file.is_relative() {
                current_dir().join(log_file)
            } else {
                log_file
            };
            logger_config.file = full_log_file
                .file_name()
                .map(|name| Path::new(name).to_path_buf())
                .unwrap_or_else(|| panic!("failed to get the filename for log_file"));
            logger_config.log_dir = full_log_file
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| panic!("failed to get the parent path for log_file"));
            logger_config.log_to_file = true;
        } else {
            logger_config.log_to_file = false;
        }
        ckb_logger_service::init(None, logger_config)
            .unwrap_or_else(|err| panic!("failed to init the logger service since {}", err))
    };

    if matches.is_present("list-specs") {
        list_specs();
        return;
    }

    *BINARY_PATH.lock() = binary;
    *VENDOR_PATH.lock() = vendor;
    PORT_COUNTER.store(start_port, Ordering::SeqCst);
    info!("binary: {}", global::binary().to_string_lossy());
    info!("vendor dir: {}", global::vendor().to_string_lossy());
    info!("start port: {}", PORT_COUNTER.load(Ordering::SeqCst));
    info!("max time: {:?}", max_time);

    let specs = filter_specs(all_specs(), spec_names_to_run);
    let total = specs.len();
    let worker_count = min(worker_count, total);
    let specs = Arc::new(Mutex::new(specs));
    let start_time = Instant::now();
    let mut spec_errors: Vec<Option<Box<dyn Any + Send>>> = Vec::new();
    let mut error_spec_names = Vec::new();

    let (notify_tx, notify_rx) = unbounded();

    info!("start {} workers...", worker_count);
    let mut workers = Workers::new(worker_count, Arc::clone(&specs), notify_tx, start_port);
    workers.start();

    let mut rerun_specs = Vec::new();
    let mut test_results = Vec::new();
    let mut worker_running = worker_count;
    let mut done_specs = 0;
    while worker_running > 0 {
        if max_time > 0 && start_time.elapsed().as_secs() > max_time {
            // shutdown, specs running to long
            workers.shutdown();
        }

        let msg = match notify_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(msg) => msg,
            Err(err) => {
                if err.is_timeout() {
                    continue;
                }
                std::panic::panic_any(err);
            }
        };
        match msg {
            Notify::Start { spec_name } => {
                info!("[{}] Start executing", spec_name);
            }
            Notify::Error {
                spec_error,
                spec_name,
                seconds,
                node_log_paths,
            } => {
                test_results.push(TestResult {
                    spec_name: spec_name.clone(),
                    status: TestResultStatus::Failed,
                    duration: seconds,
                });
                error_spec_names.push(spec_name.clone());
                rerun_specs.push(spec_name.clone());
                if fail_fast {
                    workers.shutdown();
                    worker_running -= 1;
                }
                spec_errors.push(Some(spec_error));
                if verbose {
                    info!("[{}] Error", spec_name);
                    tail_node_logs(&node_log_paths);
                }
            }
            Notify::Panick {
                spec_name,
                seconds,
                node_log_paths,
            } => {
                test_results.push(TestResult {
                    spec_name: spec_name.clone(),
                    status: TestResultStatus::Panicked,
                    duration: seconds,
                });
                error_spec_names.push(spec_name.clone());
                rerun_specs.push(spec_name.clone());
                if fail_fast {
                    workers.shutdown();
                    worker_running -= 1;
                }
                spec_errors.push(None);
                if verbose {
                    info!("[{}] Panic", spec_name);
                    print_panicked_logs(&node_log_paths);
                }
            }
            Notify::Done {
                spec_name,
                seconds,
                node_paths,
            } => {
                test_results.push(TestResult {
                    spec_name: spec_name.clone(),
                    status: TestResultStatus::Passed,
                    duration: seconds,
                });
                done_specs += 1;
                info!(
                    "{}/{} .............. [{}] Done in {} seconds",
                    done_specs, total, spec_name, seconds
                );
                if clean_tmp {
                    for path in node_paths {
                        if let Err(err) = fs::remove_dir_all(&path) {
                            warn!("failed to remove directory [{:?}] since {}", path, err);
                        }
                    }
                }
            }
            Notify::Stop => {
                worker_running -= 1;
            }
        }
    }
    // join all workers threads
    workers.join_all();

    if max_time > 0 && start_time.elapsed().as_secs() > max_time {
        error!(
            "Exit ckb-test, because total running time({} seconds) exceeds limit({} seconds)",
            start_time.elapsed().as_secs(),
            max_time
        );
    }

    if report {
        print_results(test_results);
        println!("Total elapsed time: {:?}", start_time.elapsed());
    }

    rerun_specs.extend(specs.lock().iter().map(|t| t.name().to_string()));

    if rerun_specs.is_empty() {
        return;
    }

    if !spec_errors.is_empty() {
        error!("ckb-test failed on spec {}", error_spec_names.join(", "));
        log_failed_specs(&error_spec_names)
            .unwrap_or_else(|err| error!("Failed to write integration failure reason: {}", err));
        info!("You can rerun remaining specs using following command:");
    } else {
        info!("You can run the skipped specs using following command:");
    }

    info!(
        "{} --bin {} --port {} {}",
        canonicalize_path(env::args().next().unwrap_or_else(|| "ckb-test".to_string())).display(),
        canonicalize_path(global::binary()).display(),
        start_port,
        rerun_specs.join(" "),
    );

    if !spec_errors.is_empty() {
        std::process::exit(1);
    }

    drop(logger_guard);
}

fn clap_app() -> App<'static> {
    App::new("ckb-test")
        .arg(
            Arg::with_name("binary")
                .short('b')
                .long("bin")
                .takes_value(true)
                .value_name("PATH")
                .help("Path to ckb executable")
                .default_value("../target/release/ckb"),
        )
        .arg(
            Arg::with_name("port")
                .short('p')
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
        .arg(
            Arg::with_name("concurrent")
                .short('c')
                .long("concurrent")
                .takes_value(true)
                .help("The number of specs can running concurrently")
                .default_value("4"),
        )
        .arg(
            Arg::with_name("verbose")
                .long("verbose")
                .help("Show verbose log"),
        )
        .arg(
            Arg::with_name("no-fail-fast")
                .long("no-fail-fast")
                .help("Run all tests regardless of failure"),
        )
        .arg(
            Arg::with_name("no-report")
                .long("no-report")
                .help("Do not show integration test report"),
        )
        .arg(
            Arg::with_name("log-file")
                .long("log-file")
                .takes_value(true)
                .help("Write log outputs into file."),
        )
        .arg(Arg::with_name("keep-tmp-data").long("keep-tmp-data").help(
            "Keep all temporary files. Default: only keep temporary file for the failed tests.",
        ))
        .arg(Arg::with_name("vendor").long("vendor"))
}

fn filter_specs(
    mut all_specs: Vec<Box<dyn Spec>>,
    spec_names_to_run: Vec<&str>,
) -> Vec<Box<dyn Spec>> {
    if spec_names_to_run.is_empty() {
        return all_specs;
    }

    for name in spec_names_to_run.iter() {
        if !all_specs.iter().any(|spec| spec.name() == *name) {
            eprintln!("Unknown spec {}", name);
            std::process::exit(1);
        }
    }

    all_specs.retain(|spec| spec_names_to_run.contains(&spec.name()));
    all_specs
}

fn current_dir() -> PathBuf {
    env::current_dir().expect("can't get current_dir")
}

fn canonicalize_path<P: AsRef<Path>>(path: P) -> PathBuf {
    path.as_ref()
        .canonicalize()
        .unwrap_or_else(|_| path.as_ref().to_path_buf())
}

fn all_specs() -> Vec<Box<dyn Spec>> {
    let mut specs: Vec<Box<dyn Spec>> = vec![
        Box::new(BlockSyncFromOne),
        Box::new(BlockSyncForks),
        Box::new(BlockSyncDuplicatedAndReconnect),
        Box::new(BlockSyncOrphanBlocks),
        Box::new(BlockSyncWithUncle),
        Box::new(BlockSyncNonAncestorBestBlocks),
        Box::new(RequestUnverifiedBlocks),
        Box::new(SyncTimeout),
        Box::new(GetBlockFilterCheckPoints),
        Box::new(GetBlockFilterHashes),
        Box::new(GetBlockFilters),
        Box::new(GetBlockFiltersNotReachBatch),
        Box::new(GetBlocksTimeout),
        Box::new(ChainContainsInvalidBlock),
        Box::new(ForkContainsInvalidBlock),
        Box::new(ChainFork1),
        Box::new(ChainFork2),
        Box::new(ChainFork3),
        Box::new(ChainFork4),
        Box::new(ChainFork5),
        Box::new(ChainFork6),
        Box::new(ChainFork7),
        Box::new(LongForks),
        Box::new(ForksContainSameTransactions),
        Box::new(ForksContainSameUncle),
        Box::new(WithdrawDAO),
        Box::new(WithdrawDAOWithOverflowCapacity),
        Box::new(DAOWithSatoshiCellOccupied),
        Box::new(SpendSatoshiCell::new()),
        Box::new(MiningBasic),
        Box::new(BlockTemplates),
        Box::new(BootstrapCellbase),
        Box::new(DeclaredWrongCycles),
        Box::new(DeclaredWrongCyclesChunk),
        Box::new(DeclaredWrongCyclesAndRelayAgain),
        Box::new(OrphanTxAccepted),
        Box::new(OrphanTxRejected),
        Box::new(GetRawTxPool),
        Box::new(PoolReconcile),
        Box::new(PoolResurrect),
        Box::new(InvalidHeaderDep),
        #[cfg(not(target_os = "windows"))]
        Box::new(PoolPersisted),
        Box::new(TransactionRelayBasic),
        Box::new(TransactionRelayLowFeeRate),
        Box::new(TooManyUnknownTransactions),
        // TODO failed on poor CI server
        // Box::new(TransactionRelayMultiple),
        Box::new(RelayInvalidTransaction),
        Box::new(TransactionRelayTimeout),
        Box::new(TransactionRelayEmptyPeers),
        Box::new(Discovery),
        Box::new(Disconnect),
        Box::new(MalformedMessage),
        Box::new(DepentTxInSameBlock),
        // TODO enable these after proposed/pending pool tip verify logic changing
        // Box::new(CellbaseMaturity),
        Box::new(ValidSince),
        Box::new(SendLowFeeRateTx),
        Box::new(SendLargeCyclesTxInBlock::new()),
        Box::new(SendLargeCyclesTxToRelay::new()),
        Box::new(NotifyLargeCyclesTx::new()),
        Box::new(LoadProgramFailedTx::new()),
        Box::new(RelayWithWrongTx::new()),
        Box::new(TxsRelayOrder),
        Box::new(SendTxChain),
        Box::new(DifferentTxsWithSameInput),
        Box::new(CompactBlockEmpty),
        Box::new(CompactBlockEmptyParentUnknown),
        Box::new(CompactBlockPrefilled),
        Box::new(CompactBlockMissingFreshTxs),
        Box::new(CompactBlockMissingNotFreshTxs),
        Box::new(CompactBlockMissingWithDropTx),
        Box::new(CompactBlockLoseGetBlockTransactions),
        Box::new(CompactBlockRelayParentOfOrphanBlock),
        Box::new(CompactBlockRelayLessThenSharedBestKnown),
        Box::new(InvalidLocatorSize),
        Box::new(SizeLimit),
        Box::new(CyclesLimit),
        Box::new(SendDefectedBinary::new(
            "send_defected_binary_reject_known_bugs",
            true,
        )),
        Box::new(SendDefectedBinary::new(
            "send_defected_binary_do_not_reject_known_bugs",
            false,
        )),
        Box::new(SendSecpTxUseDepGroup::new(
            "send_secp_tx_use_dep_group_data_hash",
            ScriptHashType::Data,
        )),
        Box::new(SendSecpTxUseDepGroup::new(
            "send_secp_tx_use_dep_group_type_hash",
            ScriptHashType::Type,
        )),
        Box::new(SendMultiSigSecpTxUseDepGroup::new(
            "send_multisig_secp_tx_use_dep_group_data_hash",
            ScriptHashType::Data,
        )),
        Box::new(SendMultiSigSecpTxUseDepGroup::new(
            "send_multisig_secp_tx_use_dep_group_type_hash",
            ScriptHashType::Type,
        )),
        Box::new(CheckTypical2In2OutTx::default()),
        Box::new(AlertPropagation::default()),
        // TODO These cases will fail occasionally because of some unknown
        // asynchronous issues.
        Box::new(IBDProcess),
        Box::new(WhitelistOnSessionLimit),
        // Box::new(IBDProcessWithWhiteList),
        Box::new(MalformedMessageWithWhitelist),
        // Box::new(InsufficientReward),
        Box::new(UncleInheritFromForkBlock),
        Box::new(UncleInheritFromForkUncle),
        Box::new(PackUnclesIntoEpochStarting),
        Box::new(FeeOfTransaction),
        Box::new(FeeOfMaxBlockProposalsLimit),
        Box::new(FeeOfMultipleMaxBlockProposalsLimit),
        Box::new(ProposeButNotCommit),
        Box::new(ProposeDuplicated),
        Box::new(ForkedTransaction),
        Box::new(MissingUncleRequest),
        Box::new(HandlingDescendantsOfProposed),
        Box::new(HandlingDescendantsOfCommitted),
        Box::new(ProposeOutOfOrder),
        Box::new(SubmitTransactionWhenItsParentInGap),
        Box::new(SubmitTransactionWhenItsParentInProposed),
        Box::new(ProposeTransactionButParentNot),
        Box::new(ProposalExpireRuleForCommittingAndExpiredAtOneTime),
        Box::new(ReorgHandleProposals),
        Box::new(TransactionHashCollisionDifferentWitnessHashes),
        Box::new(DuplicatedTransaction),
        Box::new(ConflictInPending),
        Box::new(ConflictInGap),
        Box::new(ConflictInProposed),
        Box::new(RemoveConflictFromPending),
        Box::new(SubmitConflict),
        Box::new(DAOVerify),
        Box::new(AvoidDuplicatedProposalsWithUncles),
        Box::new(BlockSyncRelayerCollaboration),
        Box::new(RpcGetBlockTemplate),
        Box::new(RpcSubmitBlock),
        Box::new(RpcGetBlockchainInfo),
        Box::new(RpcTruncate),
        Box::new(RpcTransactionProof),
        Box::new(RpcGetBlockMedianTime),
        #[cfg(target_os = "linux")]
        Box::new(RpcSetBan),
        Box::new(SyncTooNewBlock),
        Box::new(RelayTooNewBlock),
        Box::new(LastCommonHeaderForPeerWithWorseChain),
        Box::new(BlockTransactionsRelayParentOfOrphanBlock),
        Box::new(CellBeingSpentThenCellDepInSameBlockTestSubmitBlock),
        Box::new(CellBeingCellDepThenSpentInSameBlockTestSubmitBlock),
        Box::new(CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplate),
        Box::new(CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplateMultiple),
        Box::new(HeaderSyncCycle),
        Box::new(InboundSync),
        Box::new(OutboundSync),
        Box::new(InboundMinedDuringSync),
        Box::new(OutboundMinedDuringSync),
        Box::new(ProposalRespondSizelimit),
        Box::new(RemoveTx),
        // Test hard fork features
        Box::new(CheckCellDeps),
        Box::new(CheckAbsoluteEpochSince),
        Box::new(CheckRelativeEpochSince),
        Box::new(CheckVmVersion),
        Box::new(CheckVmBExtension),
    ];
    specs.shuffle(&mut thread_rng());
    specs
}

fn list_specs() {
    let all_specs = all_specs();
    let mut names: Vec<_> = all_specs.iter().map(|spec| spec.name()).collect();
    names.sort_unstable();
    for spec_name in names {
        println!("{}", spec_name);
    }
}

// sed -n ${{panic_ln-300}},${{panic_ln+300}}p $node_log_path
fn print_panicked_logs(node_log_paths: &[PathBuf]) {
    for (i, node_log) in node_log_paths.iter().enumerate() {
        let log_reader =
            BufReader::new(File::open(node_log).expect("failed to read node's log")).lines();
        let panic_ln = log_reader.enumerate().find(|(_ln, line)| {
            line.as_ref()
                .map(|line| line.contains("panicked at"))
                .unwrap_or(false)
        });
        if panic_ln.is_none() {
            continue;
        }

        let panic_ln = panic_ln.unwrap().0;
        let print_lns = 600;
        let from_ln = panic_ln.saturating_sub(print_lns / 2) + 1;
        println!(
            "\n************** (Node.{}) sed -n {},{}p {}",
            i,
            from_ln,
            from_ln + print_lns,
            node_log.display(),
        );
        BufReader::new(File::open(&node_log).expect("failed to read node's log"))
            .lines()
            .skip(from_ln)
            .take(print_lns)
            .for_each(|line| {
                if let Ok(line) = line {
                    println!("{}", line);
                }
            });
    }
}

// tail -n 2000 $node_log_path
fn tail_node_logs(node_log_paths: &[PathBuf]) {
    let tail_n: usize = env::var("CKB_TEST_TAIL_N")
        .unwrap_or_default()
        .parse()
        .unwrap_or(2000);
    if tail_n == 0 {
        return;
    }

    for (i, node_log) in node_log_paths.iter().enumerate() {
        let content = read_to_string(node_log).expect("failed to read node's log");
        let skip = content.lines().count().saturating_sub(tail_n);

        println!(
            "\n************** (Node.{}) tail -n {} {}",
            i,
            tail_n,
            node_log.display()
        );
        for log in content.lines().skip(skip) {
            println!("{}", log);
        }
    }
}

fn log_failed_specs(error_spec_names: &[String]) -> Result<(), io::Error> {
    let path = if let Ok(path) = env::var("CKB_INTEGRATION_FAILURE_FILE") {
        path
    } else {
        return Ok(());
    };

    let mut f = File::create(&path)?;
    for name in error_spec_names {
        writeln!(&mut f, "ckb-test failed on spec {}", name)?;
    }

    Ok(())
}

fn print_results(mut test_results: Vec<TestResult>) {
    println!("{}", "-".repeat(20));
    println!("{:50} | {:10} | {:10}", "TEST", "STATUS", "DURATION");

    test_results.sort_by(|a, b| (&a.status, a.duration).cmp(&(&b.status, b.duration)));

    for result in test_results.iter() {
        println!(
            "{:50} | {:10} | {:<10}",
            result.spec_name,
            format_args!("{:?}", result.status),
            format_args!("{} s", result.duration),
        );
    }
}
