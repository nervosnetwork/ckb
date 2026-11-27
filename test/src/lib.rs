#![allow(missing_docs)]

extern crate core;

pub mod assertion;
pub mod generic;
pub mod global;
mod logger;
mod net;
mod node;
mod rpc;
pub mod specs;
mod txo;
pub mod util;
pub mod utils;
pub mod worker;

use ckb_types::core::BlockNumber;

pub use net::Net;
pub use node::Node;
pub use specs::{Setup, Spec};
pub use txo::{TXO, TXOSet};

// ckb doesn't support tx proposal window configuration, use a hardcoded value for integration test.
pub const DEFAULT_TX_PROPOSAL_WINDOW: (BlockNumber, BlockNumber) = (2, 10);
pub const FINALIZATION_DELAY_LENGTH: BlockNumber = DEFAULT_TX_PROPOSAL_WINDOW.1 + 1;
pub const SYSTEM_CELL_ALWAYS_SUCCESS_INDEX: u32 = 5;
pub const SYSTEM_CELL_ALWAYS_FAILURE_INDEX: u32 = 6;

use crate::global::OBFS4PROXY_BINARY_PATH;
use crate::specs::*;
use crate::{
    global::{BINARY_PATH, PORT_COUNTER, VENDOR_PATH},
    worker::{
        Notify, SEQUENCIAL_TASKS, ScheduledSpec, WorkerConfig, Workers, discover_spec_log_paths,
    },
};
use ckb_channel::unbounded;
use ckb_logger::{debug, error, info, warn};
use ckb_types::core::ScriptHashType;
use ckb_util::Mutex;
use clap::{Arg, Command};
use rand::{SeedableRng, rngs::StdRng, seq::SliceRandom};
use std::any::Any;
use std::cmp::{Reverse, min};
use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs::{self, File, read_to_string};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Copy)]
enum TestResultStatus {
    Passed,
    Failed,
    Panicked,
}

struct TestResult {
    spec_name: String,
    base_name: String,
    status: TestResultStatus,
    duration: Duration,
    detail: Option<String>,
    spec_dir: Option<PathBuf>,
    report_path: Option<PathBuf>,
    runner_log_path: Option<PathBuf>,
    node_log_paths: Vec<PathBuf>,
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).clamp(4, 8))
        .unwrap_or(4)
}

struct RunningSpec {
    spec_name: String,
    base_name: String,
    started_at: Instant,
    spec_dir: PathBuf,
}

fn next_coordinator_poll_timeout(
    now: Instant,
    start_time: Instant,
    max_time: u64,
    spec_timeout: u64,
    running: &[RunningSpec],
) -> Duration {
    let mut wait = Duration::from_secs(5);
    if max_time > 0 {
        let max_deadline = start_time + Duration::from_secs(max_time);
        wait = wait.min(max_deadline.saturating_duration_since(now));
    }
    if spec_timeout > 0 {
        let limit = Duration::from_secs(spec_timeout);
        for item in running {
            let spec_deadline = item.started_at + limit;
            wait = wait.min(spec_deadline.saturating_duration_since(now));
        }
    }
    wait
}

pub(crate) fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs >= 60.0 {
        let mins = (secs / 60.0).floor() as u64;
        let rem = secs - (mins as f64 * 60.0);
        format!("{mins}m {rem:04.1}s")
    } else if secs >= 10.0 {
        format!("{secs:.1}s")
    } else {
        format!("{secs:.2}s")
    }
}

pub(crate) fn extract_panic_message(payload: &(dyn Any + Send)) -> String {
    let raw = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    };
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

fn parse_shard(raw: &str) -> Result<(usize, usize), String> {
    let (idx_str, total_str) = raw
        .split_once('/')
        .ok_or_else(|| format!("invalid --shard '{raw}': expected INDEX/TOTAL (e.g. 0/8)"))?;
    let index: usize = idx_str.parse().map_err(|_| {
        format!("invalid --shard index '{idx_str}': must be a non-negative integer")
    })?;
    let total: usize = total_str
        .parse()
        .map_err(|_| format!("invalid --shard total '{total_str}': must be a positive integer"))?;
    if total == 0 {
        return Err("invalid --shard: TOTAL must be >= 1".to_string());
    }
    if index >= total {
        return Err(format!(
            "invalid --shard '{raw}': INDEX ({index}) must be < TOTAL ({total}) (0-based)"
        ));
    }
    Ok((index, total))
}

#[allow(clippy::cognitive_complexity)]
pub fn main_test() {
    unsafe {
        env::set_var("RUST_BACKTRACE", "full");
    }

    let clap_app = clap_app();
    let matches = clap_app.get_matches();

    let binary = matches.get_one::<PathBuf>("binary").cloned().unwrap();
    let obfs4proxy_binary = matches
        .get_one::<PathBuf>("obfs4proxy-bin")
        .cloned()
        .unwrap();
    let start_port = matches.get_one::<u16>("port").copied();
    let spec_names_to_run: Vec<_> = matches
        .get_many::<String>("specs")
        .unwrap_or_default()
        .map(|v| v.as_str())
        .collect();
    let spec_filter = matches.get_one::<String>("filter").map(|v| v.as_str());
    let shard = matches.get_one::<String>("shard").map(|raw| {
        parse_shard(raw).unwrap_or_else(|err| {
            eprintln!("{err}");
            std::process::exit(1);
        })
    });
    let seed = matches
        .get_one::<u64>("seed")
        .copied()
        .unwrap_or_else(rand::random);
    let repeat = matches.get_one::<usize>("repeat").cloned().unwrap();
    if repeat == 0 {
        eprintln!("--repeat must be >= 1");
        std::process::exit(1);
    }
    let max_time = matches.get_one::<u64>("max-time").cloned().unwrap();
    let spec_timeout = matches.get_one::<u64>("spec-timeout").cloned().unwrap();
    let worker_count = matches
        .get_one::<usize>("concurrent")
        .copied()
        .unwrap_or_else(default_concurrency);
    if worker_count == 0 {
        eprintln!("--concurrent must be >= 1");
        std::process::exit(1);
    }
    let vendor = matches
        .get_one::<PathBuf>("vendor")
        .cloned()
        .unwrap_or_else(|| current_dir().join("vendor"));
    let log_file_opt = matches.get_one::<PathBuf>("log-file").cloned();
    let json_report_opt = matches.get_one::<PathBuf>("json-report").cloned();
    let fail_fast = !matches.get_flag("no-fail-fast");
    let report = !matches.get_flag("no-report");
    let clean_tmp = !matches.get_flag("keep-tmp-data");
    let pause_on_fail = matches.get_flag("pause-on-fail");
    let verbose = matches.get_flag("verbose");

    {
        let filter = if !verbose {
            env::var("RUST_LOG").unwrap_or_else(|_| {
                "warn,ckb_network=error,ckb_test=warn,ckb_test::progress=info".to_string()
            })
        } else {
            format!(
                "{},{}=trace",
                env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
                module_path!(),
            )
        };
        let full_log_file = log_file_opt.map(|log_file| {
            if log_file.is_relative() {
                current_dir().join(log_file)
            } else {
                log_file
            }
        });
        logger::init_test_logger(&filter, full_log_file.as_deref());
    }

    if matches.get_flag("list-specs") {
        list_specs(spec_filter, shard);
        return;
    }

    *BINARY_PATH.lock() = binary;
    *OBFS4PROXY_BINARY_PATH.lock() = obfs4proxy_binary;
    *VENDOR_PATH.lock() = vendor;
    if let Some(start_port) = start_port {
        global::set_explicit_start_port(start_port);
    }

    let mut rng = StdRng::seed_from_u64(seed);
    let selected_base = select_specs(all_specs(), &spec_names_to_run, spec_filter, shard);
    let mut specs: Vec<ScheduledSpec> = selected_base
        .into_iter()
        .map(|spec| ScheduledSpec::new(spec, 1, repeat))
        .collect();
    for iter in 2..=repeat {
        let extra = select_specs(all_specs(), &spec_names_to_run, spec_filter, shard);
        specs.extend(
            extra
                .into_iter()
                .map(|spec| ScheduledSpec::new(spec, iter, repeat)),
        );
    }
    if specs.is_empty() {
        warn!(
            target: "ckb_test::progress",
            "No specs matched the given selection (filter={spec_filter:?}, shard={shard:?})"
        );
        return;
    }
    specs.shuffle(&mut rng);
    // Schedule longest-running specs first (workers pop from the end of `specs`)
    // to minimize parallel tail latency while keeping equal-weight specs randomized by `seed`.
    specs.sort_by_key(|task| spec_schedule_weight(&task.base_name));

    let running_specs: Arc<Mutex<Vec<RunningSpec>>> = Arc::new(Mutex::new(Vec::new()));
    let effective_start_port = PORT_COUNTER.load(Ordering::SeqCst);
    let rerun_cmd_prefix = if let Some(port) = start_port {
        format!(
            "{} --bin {} --obfs4proxy-bin {} --port {}",
            canonicalize_path(env::args().next().unwrap_or_else(|| "ckb-test".to_string()))
                .display(),
            canonicalize_path(global::binary()).display(),
            canonicalize_path(global::obfs4proxy_binary()).display(),
            port,
        )
    } else {
        format!(
            "{} --bin {} --obfs4proxy-bin {}",
            canonicalize_path(env::args().next().unwrap_or_else(|| "ckb-test".to_string()))
                .display(),
            canonicalize_path(global::binary()).display(),
            canonicalize_path(global::obfs4proxy_binary()).display(),
        )
    };

    let total = specs.len();
    let worker_count = min(worker_count, total);
    let shard_desc = shard
        .map(|(i, n)| format!(", shard={i}/{n}"))
        .unwrap_or_default();
    let spec_timeout_desc = if spec_timeout > 0 {
        format!(", spec_timeout={spec_timeout}s")
    } else {
        String::new()
    };
    info!(
        target: "ckb_test::progress",
        "ckb-test: running {} spec(s) with {} worker(s) (seed={}, port={}, repeat={}, max_time={}s{}{}, bin={})",
        total,
        worker_count,
        seed,
        effective_start_port,
        repeat,
        max_time,
        shard_desc,
        spec_timeout_desc,
        global::binary().to_string_lossy()
    );
    debug!(
        "obfs4proxy binary: {}, vendor dir: {}",
        global::obfs4proxy_binary().to_string_lossy(),
        global::vendor().to_string_lossy()
    );

    let specs = Arc::new(Mutex::new(specs));
    let start_time = Instant::now();
    let mut spec_errors: Vec<Option<Box<dyn Any + Send>>> = Vec::new();
    let mut error_spec_names = Vec::new();
    let mut timed_out_specs = Vec::new();

    let (notify_tx, notify_rx) = unbounded();

    let cloned_running = Arc::clone(&running_specs);
    ctrlc::set_handler(move || {
        let running = cloned_running.lock();
        warn!(
            target: "ckb_test::progress",
            "Interrupted: {} spec(s) still running",
            running.len()
        );
        for item in running.iter() {
            warn!(
                target: "ckb_test::progress",
                "  still running: {} ({}) dir={}",
                item.spec_name,
                format_duration(item.started_at.elapsed()),
                item.spec_dir.display()
            );
        }
        drop(running);
        node::kill_all_active_children();
        log::logger().flush();
        std::process::exit(130);
    })
    .expect("Error setting Ctrl-C handler");

    let worker_config = WorkerConfig {
        seed,
        pause_on_fail,
        rerun_cmd_prefix: rerun_cmd_prefix.clone(),
    };
    let mut workers = Workers::new(worker_count, Arc::clone(&specs), notify_tx, worker_config);
    workers.start();

    let mut rerun_specs = Vec::new();
    let mut test_results = Vec::new();
    let mut worker_running = worker_count;
    let mut completed_specs = 0;
    let total_width = total.to_string().len();
    let mut started_sequential = false;
    let mut shutting_down = false;
    let mut hit_max_time = false;
    let mut last_progress = Instant::now();
    let mut last_heartbeat = Instant::now();
    while worker_running > 0 {
        if max_time > 0 && start_time.elapsed() >= Duration::from_secs(max_time) {
            hit_max_time = true;
            let active: Vec<(String, String, Duration, PathBuf)> = running_specs
                .lock()
                .drain(..)
                .map(|item| {
                    (
                        item.spec_name,
                        item.base_name,
                        item.started_at.elapsed(),
                        item.spec_dir,
                    )
                })
                .collect();
            for (spec_name, base_name, elapsed, dir) in active {
                let reason = format!(
                    "exceeded --max-time {}s (elapsed {})",
                    max_time,
                    format_duration(elapsed)
                );
                let logs = discover_spec_log_paths(&dir, &[]);
                let runner_log_path = logger::read_runner_log_path(&dir);
                error!(
                    target: "ckb_test::progress",
                    "TIMEOUT {} {} (dir={})",
                    spec_name,
                    reason,
                    dir.display()
                );
                test_results.push(TestResult {
                    spec_name,
                    base_name: base_name.clone(),
                    status: TestResultStatus::Failed,
                    duration: elapsed,
                    detail: Some(reason),
                    spec_dir: Some(dir),
                    report_path: None,
                    runner_log_path,
                    node_log_paths: logs,
                });
                error_spec_names.push(base_name.clone());
                rerun_specs.push(base_name);
                spec_errors.push(None);
            }
            workers.shutdown();
            node::kill_all_active_children();
            break;
        }

        if spec_timeout > 0 {
            let overdue: Vec<(String, String, Duration, PathBuf)> = {
                let mut running = running_specs.lock();
                let mut found = Vec::new();
                running.retain(|item| {
                    let elapsed = item.started_at.elapsed();
                    if elapsed >= Duration::from_secs(spec_timeout) {
                        found.push((
                            item.spec_name.clone(),
                            item.base_name.clone(),
                            elapsed,
                            item.spec_dir.clone(),
                        ));
                        false
                    } else {
                        true
                    }
                });
                found
            };
            if !overdue.is_empty() {
                for (name, base_name, elapsed, dir) in overdue {
                    let reason = format!(
                        "exceeded --spec-timeout {}s (elapsed {})",
                        spec_timeout,
                        format_duration(elapsed)
                    );
                    error!(
                        target: "ckb_test::progress",
                        "TIMEOUT {} {} (dir={})",
                        name,
                        reason,
                        dir.display()
                    );
                    let logs = discover_spec_log_paths(&dir, &[]);
                    let runner_log_path = logger::read_runner_log_path(&dir);
                    tail_node_logs(&logs);
                    test_results.push(TestResult {
                        spec_name: name.clone(),
                        base_name: base_name.clone(),
                        status: TestResultStatus::Failed,
                        duration: elapsed,
                        detail: Some(reason),
                        spec_dir: Some(dir),
                        report_path: None,
                        runner_log_path,
                        node_log_paths: logs,
                    });
                    timed_out_specs.push(name);
                    error_spec_names.push(base_name.clone());
                    rerun_specs.push(base_name);
                    spec_errors.push(None);
                }
                workers.shutdown();
                node::kill_all_active_children();
                break;
            }
        }

        if worker_running == 1 && !started_sequential && !shutting_down {
            started_sequential = true;
            if workers.has_sequential_tasks() {
                workers.start_sequencial()
            }
        }

        let poll_timeout = {
            let running = running_specs.lock();
            next_coordinator_poll_timeout(
                Instant::now(),
                start_time,
                max_time,
                spec_timeout,
                &running,
            )
        };
        let msg = match notify_rx.recv_timeout(poll_timeout) {
            Ok(msg) => msg,
            Err(err) => {
                if err.is_timeout() {
                    if last_progress.elapsed() >= Duration::from_secs(30)
                        && last_heartbeat.elapsed() >= Duration::from_secs(30)
                    {
                        last_heartbeat = Instant::now();
                        let running_summary: Vec<String> = running_specs
                            .lock()
                            .iter()
                            .map(|item| {
                                format!(
                                    "{} ({})",
                                    item.spec_name,
                                    format_duration(item.started_at.elapsed())
                                )
                            })
                            .collect();
                        if !running_summary.is_empty() {
                            warn!(
                                target: "ckb_test::progress",
                                "still running ({} spec(s)): {}",
                                running_summary.len(),
                                running_summary.join(", ")
                            );
                        }
                    }
                    continue;
                }
                std::panic::panic_any(err);
            }
        };
        match msg {
            Notify::Start {
                spec_name,
                base_name,
                spec_dir,
            } => {
                debug!(
                    "[{}] Start executing (dir={})",
                    spec_name,
                    spec_dir.display()
                );
                running_specs.lock().push(RunningSpec {
                    spec_name,
                    base_name,
                    started_at: Instant::now(),
                    spec_dir,
                });
            }
            Notify::Error {
                spec_error,
                spec_name,
                base_name,
                duration,
                spec_dir,
                report_path,
                runner_log_path,
                node_log_paths,
                node_paths,
            } => {
                last_progress = Instant::now();
                let reason = extract_panic_message(spec_error.as_ref());
                completed_specs += 1;
                error!(
                    target: "ckb_test::progress",
                    "FAIL  [{completed_specs:>total_width$}/{total}] {:>7}  {} — {}",
                    format_duration(duration),
                    spec_name,
                    reason
                );
                // Always preserve temporary directories & logs for failed specs for post-mortem debugging.
                let preserved_dir = spec_dir.map(utils::TempPathBuf::leak);
                for path in node_paths {
                    path.leak();
                }
                test_results.push(TestResult {
                    spec_name: spec_name.clone(),
                    base_name: base_name.clone(),
                    status: TestResultStatus::Failed,
                    duration,
                    detail: Some(reason),
                    spec_dir: preserved_dir,
                    report_path,
                    runner_log_path,
                    node_log_paths: node_log_paths.clone(),
                });
                error_spec_names.push(base_name.clone());
                rerun_specs.push(base_name);
                if fail_fast {
                    workers.shutdown();
                    shutting_down = true;
                }
                remove_running_spec(&running_specs, &spec_name);
                spec_errors.push(Some(spec_error));
                if verbose {
                    info!(target: "ckb_test::progress", "[{}] Error", spec_name);
                    tail_node_logs(&node_log_paths);
                }
            }
            Notify::Panick {
                spec_name,
                base_name,
                duration,
                spec_dir,
                report_path,
                runner_log_path,
                node_log_paths,
                node_paths,
            } => {
                last_progress = Instant::now();
                completed_specs += 1;
                error!(
                    target: "ckb_test::progress",
                    "PANIC [{completed_specs:>total_width$}/{total}] {:>7}  {} (node panicked)",
                    format_duration(duration),
                    spec_name
                );
                // Always preserve temporary directories & logs for panicked specs for post-mortem debugging.
                let preserved_dir = spec_dir.leak();
                for path in node_paths {
                    path.leak();
                }
                test_results.push(TestResult {
                    spec_name: spec_name.clone(),
                    base_name: base_name.clone(),
                    status: TestResultStatus::Panicked,
                    duration,
                    detail: Some("ckb node process panicked (check node log)".to_string()),
                    spec_dir: Some(preserved_dir),
                    report_path,
                    runner_log_path,
                    node_log_paths: node_log_paths.clone(),
                });
                error_spec_names.push(base_name.clone());
                rerun_specs.push(base_name);
                if fail_fast {
                    workers.shutdown();
                    shutting_down = true;
                }
                spec_errors.push(None);
                remove_running_spec(&running_specs, &spec_name);
                print_panicked_logs(&node_log_paths);
            }
            Notify::Done {
                spec_name,
                base_name,
                duration,
                spec_dir,
                node_paths,
            } => {
                last_progress = Instant::now();
                remove_running_spec(&running_specs, &spec_name);
                completed_specs += 1;
                if spec_timeout > 0 && duration >= Duration::from_secs(spec_timeout) {
                    let reason = format!(
                        "exceeded --spec-timeout {}s (completed in {})",
                        spec_timeout,
                        format_duration(duration)
                    );
                    let preserved_dir = spec_dir.leak();
                    for path in node_paths {
                        path.leak();
                    }
                    let logs = discover_spec_log_paths(&preserved_dir, &[]);
                    let runner_log_path = logger::read_runner_log_path(&preserved_dir);
                    error!(
                        target: "ckb_test::progress",
                        "TIMEOUT [{completed_specs:>total_width$}/{total}] {:>7}  {} — {}",
                        format_duration(duration),
                        spec_name,
                        reason
                    );
                    test_results.push(TestResult {
                        spec_name: spec_name.clone(),
                        base_name: base_name.clone(),
                        status: TestResultStatus::Failed,
                        duration,
                        detail: Some(reason),
                        spec_dir: Some(preserved_dir),
                        report_path: None,
                        runner_log_path,
                        node_log_paths: logs,
                    });
                    error_spec_names.push(base_name.clone());
                    rerun_specs.push(base_name);
                    spec_errors.push(None);
                    if fail_fast {
                        workers.shutdown();
                        shutting_down = true;
                    }
                } else {
                    test_results.push(TestResult {
                        spec_name: spec_name.clone(),
                        base_name,
                        status: TestResultStatus::Passed,
                        duration,
                        detail: None,
                        spec_dir: None,
                        report_path: None,
                        runner_log_path: None,
                        node_log_paths: Vec::new(),
                    });
                    info!(
                        target: "ckb_test::progress",
                        "PASS  [{completed_specs:>total_width$}/{total}] {:>7}  {}",
                        format_duration(duration),
                        spec_name
                    );
                    if clean_tmp {
                        for path in node_paths {
                            if path.is_owned_root()
                                && let Err(err) = fs::remove_dir_all(&path)
                            {
                                warn!("failed to remove directory [{:?}] since {}", path, err);
                            }
                        }
                        if let Err(err) = fs::remove_dir_all(&spec_dir) {
                            warn!(
                                "failed to remove spec directory [{:?}] since {}",
                                spec_dir, err
                            );
                        }
                    } else {
                        // `--keep-tmp-data` is set: preserve passed spec directories too.
                        spec_dir.leak();
                        for path in node_paths {
                            path.leak();
                        }
                    }
                }
            }
            Notify::Stop => {
                worker_running -= 1;
            }
        }
    }

    let total_elapsed = start_time.elapsed();
    let aborted_by_timeout = !timed_out_specs.is_empty() || hit_max_time;
    if !timed_out_specs.is_empty() {
        error!(
            target: "ckb_test::progress",
            "Exiting immediately due to --spec-timeout ({}s) on: {}",
            spec_timeout,
            timed_out_specs.join(", ")
        );
    }
    if hit_max_time {
        error!(
            target: "ckb_test::progress",
            "Exit ckb-test, because total running time({}) exceeds --max-time({}s)",
            format_duration(total_elapsed),
            max_time
        );
    }

    if !aborted_by_timeout {
        // join all workers threads
        workers.join_all();
    } else {
        // Ensure every active child process has been terminated & reaped before exiting.
        node::kill_all_active_children();
    }

    log::logger().flush();

    if report {
        print_results(&mut test_results, total_elapsed, seed, repeat);
    } else if !spec_errors.is_empty() || aborted_by_timeout {
        print_failures(&test_results);
    }

    if let Some(json_path) = json_report_opt.as_deref() {
        write_json_report(json_path, &test_results, total_elapsed, seed);
    }

    rerun_specs.extend(specs.lock().iter().map(|t| t.base_name.clone()));
    let mut seen_rerun = HashSet::new();
    rerun_specs.retain(|name| seen_rerun.insert(name.clone()));

    let has_failures = !spec_errors.is_empty() || aborted_by_timeout;
    if !has_failures && rerun_specs.is_empty() {
        return;
    }

    if has_failures {
        if !error_spec_names.is_empty() {
            eprintln!("\nckb-test failed on spec {}", error_spec_names.join(", "));
            if let Err(err) = log_failed_specs(&error_spec_names) {
                eprintln!("Failed to write integration failure reason: {err}");
            }
        }
        if !rerun_specs.is_empty() {
            println!(
                "You can rerun remaining specs (add --verbose or --pause-on-fail for debugging) using:"
            );
            println!(
                "{} --seed {} {}",
                rerun_cmd_prefix,
                seed,
                rerun_specs.join(" "),
            );
        }
        log::logger().flush();
        std::process::exit(1);
    }

    println!("\nYou can run the skipped specs using following command:");
    println!(
        "{} --seed {} {}",
        rerun_cmd_prefix,
        seed,
        rerun_specs.join(" "),
    );
}

fn clap_app() -> Command {
    Command::new("ckb-test")
        .arg(
            Arg::new("binary")
                .short('b')
                .long("bin")
                .action(clap::ArgAction::Set)
                .value_parser(clap::value_parser!(PathBuf))
                .help("Path to ckb executable")
                .default_value("../target/release/ckb"),
        )
        .arg(
            Arg::new("obfs4proxy-bin")
                .long("obfs4proxy-bin")
                .action(clap::ArgAction::Set)
                .value_parser(clap::value_parser!(PathBuf))
                .help("Path to obfs4proxy executable")
                .default_value("./obfs4/obfs4proxy/obfs4proxy"),
        )
        .arg(
            Arg::new("port")
                .short('p')
                .long("port")
                .action(clap::ArgAction::Set)
                .value_parser(clap::value_parser!(u16))
                .help("Starting port number used to start ckb nodes (default: PID-spread in 10000..30000)"),
        )
        .arg(
            Arg::new("max-time")
                .long("max-time")
                .action(clap::ArgAction::Set)
                .value_parser(clap::value_parser!(u64))
                .help("Exit when total running time exceeds this limit")
                .default_value("0"),
        )
        .arg(
            Arg::new("spec-timeout")
                .long("spec-timeout")
                .action(clap::ArgAction::Set)
                .value_parser(clap::value_parser!(u64))
                .help("Abort and dump node logs if any single spec runs longer than N seconds (0 = disabled)")
                .default_value("0"),
        )
        .arg(
            Arg::new("list-specs")
                .long("list-specs")
                .action(clap::ArgAction::SetTrue)
                .help("list all specs"),
        )
        .arg(
            Arg::new("specs")
                .action(clap::ArgAction::Append)
                .help("Specs to run"),
        )
        .arg(
            Arg::new("filter")
                .short('f')
                .long("filter")
                .action(clap::ArgAction::Set)
                .help("Only run/list specs whose names contain this case-insensitive substring"),
        )
        .arg(
            Arg::new("shard")
                .long("shard")
                .action(clap::ArgAction::Set)
                .help("Run/list only shard INDEX/TOTAL (0-based, e.g. 0/8..7/8), weight-balanced across shards"),
        )
        .arg(
            Arg::new("seed")
                .long("seed")
                .action(clap::ArgAction::Set)
                .value_parser(clap::value_parser!(u64))
                .help("Random seed for spec execution order (printed at startup for reproducibility)"),
        )
        .arg(
            Arg::new("repeat")
                .long("repeat")
                .action(clap::ArgAction::Set)
                .value_parser(clap::value_parser!(usize))
                .help("Repeat selected specs N times (useful for reproducing flaky failures)")
                .default_value("1"),
        )
        .arg(
            Arg::new("concurrent")
                .short('c')
                .long("concurrent")
                .action(clap::ArgAction::Set)
                .value_parser(clap::value_parser!(usize))
                .help("The number of specs that can run concurrently (default: half of CPU cores, clamped to 4..=8)"),
        )
        .arg(
            Arg::new("verbose")
                .long("verbose")
                .action(clap::ArgAction::SetTrue)
                .help("Show verbose log"),
        )
        .arg(
            Arg::new("no-fail-fast")
                .long("no-fail-fast")
                .action(clap::ArgAction::SetTrue)
                .help("Run all tests regardless of failure"),
        )
        .arg(
            Arg::new("no-report")
                .long("no-report")
                .action(clap::ArgAction::SetTrue)
                .help("Do not show integration test report"),
        )
        .arg(
            Arg::new("log-file")
                .long("log-file")
                .action(clap::ArgAction::Set)
                .value_parser(clap::value_parser!(PathBuf))
                .help("Write log outputs into file."),
        )
        .arg(
            Arg::new("json-report")
                .long("json-report")
                .action(clap::ArgAction::Set)
                .value_parser(clap::value_parser!(PathBuf))
                .help("Write structured JSON summary report to this path"),
        )
        .arg(
            Arg::new("pause-on-fail")
                .long("pause-on-fail")
                .action(clap::ArgAction::SetTrue)
                .help("Keep failed spec nodes alive and wait for Enter before cleanup (for interactive RPC/log debugging)"),
        )
        .arg(Arg::new("keep-tmp-data").long("keep-tmp-data").action(clap::ArgAction::SetTrue).help(
            "Keep all temporary files. Default: only keep temporary file for the failed tests.",
        ))
        .arg(Arg::new("vendor").long("vendor").action(clap::ArgAction::SetTrue).value_parser(clap::value_parser!(PathBuf)))
}

fn remove_running_spec(running_specs: &Arc<Mutex<Vec<RunningSpec>>>, spec_name: &str) {
    let mut running = running_specs.lock();
    if let Some(pos) = running.iter().position(|item| item.spec_name == spec_name) {
        running.remove(pos);
    }
}

fn spec_matches_filter(name: &str, filter: &str) -> bool {
    name.to_ascii_lowercase()
        .contains(&filter.to_ascii_lowercase())
}

/// Partition items deterministically across `shard_total` buckets using greedy LPT
/// (Longest Processing Time first) bin packing by `(weight, name)` so CI shards finish
/// in balanced wall-clock time.
fn lpt_shard_indices(names: &[&str], shard_index: usize, shard_total: usize) -> HashSet<usize> {
    let mut order: Vec<usize> = (0..names.len()).collect();
    order.sort_by(|&a, &b| {
        let wa = spec_shard_weight(names[a]);
        let wb = spec_shard_weight(names[b]);
        wb.cmp(&wa).then_with(|| names[a].cmp(names[b]))
    });

    let mut bucket_loads = vec![0u64; shard_total];
    let mut bucket_counts = vec![0usize; shard_total];
    let mut selected = HashSet::new();

    for idx in order {
        let target = (0..shard_total)
            .min_by_key(|&b| (bucket_loads[b], bucket_counts[b], b))
            .expect("shard_total >= 1");
        bucket_loads[target] += u64::from(spec_shard_weight(names[idx]));
        bucket_counts[target] += 1;
        if target == shard_index {
            selected.insert(idx);
        }
    }
    selected
}

fn select_specs(
    mut all_specs: Vec<Box<dyn Spec>>,
    spec_names_to_run: &[&str],
    spec_filter: Option<&str>,
    shard: Option<(usize, usize)>,
) -> Vec<Box<dyn Spec>> {
    if !spec_names_to_run.is_empty() {
        for name in spec_names_to_run.iter() {
            if !all_specs.iter().any(|spec| spec.name() == *name) {
                eprintln!("Unknown spec {name}");
                std::process::exit(1);
            }
        }
        all_specs.retain(|spec| spec_names_to_run.contains(&spec.name()));
    }

    if let Some(pattern) = spec_filter {
        all_specs.retain(|spec| spec_matches_filter(spec.name(), pattern));
    }

    if let Some((shard_index, shard_total)) = shard {
        let names: Vec<&str> = all_specs.iter().map(|s| s.name()).collect();
        let keep = lpt_shard_indices(&names, shard_index, shard_total);
        all_specs = all_specs
            .into_iter()
            .enumerate()
            .filter_map(|(i, spec)| keep.contains(&i).then_some(spec))
            .collect();
    }

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
    let specs: Vec<Box<dyn Spec>> = vec![
        Box::new(BlockSyncFromOne),
        Box::new(BlockSyncForks),
        Box::new(BlockSyncDuplicatedAndReconnect),
        Box::new(BlockSyncOrphanBlocks),
        Box::new(BlockSyncWithUncle),
        Box::new(BlockSyncNonAncestorBestBlocks),
        Box::new(RequestUnverifiedBlocks),
        Box::new(SyncTimeout),
        Box::new(SyncInvalid),
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
        Box::new(SendConflictTxToRelay),
        Box::new(SendConflictTxToRelayRBF),
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
        Box::new(TxPoolOrphanNormal),
        Box::new(TxPoolOrphanReverse),
        Box::new(TxPoolOrphanUnordered),
        Box::new(TxPoolOrphanPartialInputUnknown),
        Box::new(TxPoolOrphanDoubleSpend),
        Box::new(OrphanTxRejected),
        Box::new(GetRawTxPool),
        Box::new(PoolReconcile),
        Box::new(PoolResurrect),
        Box::new(PoolResolveConflictAfterReorg),
        Box::new(InvalidHeaderDep),
        // TODO fix this on windows platform
        #[cfg(not(target_os = "windows"))]
        Box::new(PoolPersisted),
        Box::new(TransactionRelayBasic),
        Box::new(TransactionRelayLowFeeRate),
        Box::new(TooManyUnknownTransactions),
        Box::new(TransactionRelayMultiple),
        Box::new(RelayInvalidTransaction),
        Box::new(RelayInvalidTransactionResumable),
        Box::new(TransactionRelayTimeout),
        Box::new(TransactionRelayEmptyPeers),
        Box::new(TransactionRelayConflict),
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
        Box::new(SendTxChainRevOrder),
        Box::new(TxPoolEntryStatus),
        Box::new(DifferentTxsWithSameInputWithOutRBF),
        Box::new(RbfEnable),
        Box::new(RbfBasic),
        Box::new(RbfSameInput),
        Box::new(RbfOnlyForResolveDead),
        Box::new(RbfSameInputwithLessFee),
        Box::new(RbfTooManyDescendants),
        Box::new(RbfContainNewTx),
        Box::new(RbfContainInvalidInput),
        Box::new(RbfChildPayForParent),
        Box::new(RbfContainInvalidCells),
        Box::new(RbfRejectReplaceProposed),
        Box::new(RbfReplaceProposedSuccess),
        Box::new(RbfConcurrency),
        Box::new(RbfCellDepsCheck),
        Box::new(RbfCyclingAttack),
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
        Box::new(TxPoolLimitAncestorCount),
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
        Box::<crate::specs::CheckTypical2In2OutTx>::default(),
        Box::new(IBDProcess),
        Box::new(WhitelistOnSessionLimit),
        Box::new(IBDProcessWithWhiteList),
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
        Box::new(MalformedTx),
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
        Box::new(CheckVmVersion1),
        Box::new(CheckVmVersion2),
        Box::new(CheckVmBExtension),
        Box::new(RandomlyKill),
        Box::new(SyncChurn),
    ];
    specs
}

fn spec_schedule_weight(name: &str) -> u32 {
    match name {
        "TransactionRelayTimeout" => 37,
        "InboundMinedDuringSync"
        | "GetBlocksTimeout"
        | "InboundSync"
        | "OutboundMinedDuringSync"
        | "HeaderSyncCycle"
        | "OutboundSync" => 32,
        "Discovery" => 19,
        "RelayTooNewBlock" | "SyncTooNewBlock" => 17,
        "SendTxChain" => 15,
        "SyncChurn" => 12,
        "SyncTimeout"
        | "GetBlockFiltersNotReachBatch"
        | "MalformedMessageWithWhitelist"
        | "BlockSyncForks"
        | "BlockSyncRelayerCollaboration"
        | "RandomlyKill"
        | "ChainFork5"
        | "ChainFork6"
        | "BlockSyncOrphanBlocks"
        | "ChainFork3"
        | "FeeOfMultipleMaxBlockProposalsLimit" => 7,
        "FeeOfMaxBlockProposalsLimit"
        | "ChainFork4"
        | "ChainFork2"
        | "ChainFork7"
        | "GetBlockFilters"
        | "ForksContainSameTransactions"
        | "ChainContainsInvalidBlock"
        | "GetBlockFilterHashes"
        | "TxPoolLimitAncestorCount" => 5,
        "SyncInvalid"
        | "IBDProcess"
        | "ValidSince"
        | "DeclaredWrongCyclesAndRelayAgain"
        | "ChainFork1"
        | "SendLargeCyclesTxInBlock"
        | "InvalidLocatorSize"
        | "LongForks"
        | "RpcSubmitBlock"
        | "RpcGetBlockTemplate"
        | "CellBeingCellDepAndSpentInSameBlockTestGetBlockTemplateMultiple"
        | "SendConflictTxToRelayRBF"
        | "WhitelistOnSessionLimit"
        | "SendConflictTxToRelay"
        | "WithdrawDAOWithOverflowCapacity"
        | "CheckVmVersion2"
        | "ProposalRespondSizelimit"
        | "GetBlockFilterCheckPoints" => 3,
        _ => 2,
    }
}

/// Weight used for cross-shard LPT load balancing.
/// Sequential tasks (`SyncChurn`, `RandomlyKill`) cannot overlap with parallel specs
/// within a shard, so they get extra weight so they land in distinct CI shards.
fn spec_shard_weight(name: &str) -> u32 {
    if SEQUENCIAL_TASKS.contains(&name) {
        spec_schedule_weight(name) + 25
    } else {
        spec_schedule_weight(name)
    }
}

fn list_specs(spec_filter: Option<&str>, shard: Option<(usize, usize)>) {
    let selected = select_specs(all_specs(), &[], spec_filter, shard);
    let mut names: Vec<_> = selected.iter().map(|spec| spec.name()).collect();
    names.sort_unstable();
    for spec_name in names {
        println!("{spec_name}");
    }
}

// sed -n ${{panic_ln-300}},${{panic_ln+300}}p $node_log_path
fn print_panicked_logs(node_log_paths: &[PathBuf]) {
    for (i, node_log) in node_log_paths.iter().enumerate() {
        let Ok(file) = File::open(node_log) else {
            warn!("Node.{i} log file does not exist: {}", node_log.display());
            continue;
        };
        let log_reader = BufReader::new(file).lines();
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
        if let Ok(file) = File::open(node_log) {
            BufReader::new(file)
                .lines()
                .skip(from_ln)
                .take(print_lns)
                .for_each(|line| {
                    if let Ok(line) = line {
                        println!("{line}");
                    }
                });
        }
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
        let Ok(content) = read_to_string(node_log) else {
            warn!("Node.{i} log file does not exist: {}", node_log.display());
            continue;
        };
        let skip = content.lines().count().saturating_sub(tail_n);

        println!(
            "\n************** (Node.{}) tail -n {} {}",
            i,
            tail_n,
            node_log.display()
        );
        for log in content.lines().skip(skip) {
            println!("{log}");
        }
    }
}

fn log_failed_specs(error_spec_names: &[String]) -> Result<(), io::Error> {
    let path = if let Ok(path) = env::var("CKB_INTEGRATION_FAILURE_FILE") {
        path
    } else {
        return Ok(());
    };

    let mut f = File::create(path)?;
    for name in error_spec_names {
        writeln!(&mut f, "ckb-test failed on spec {name}")?;
    }

    Ok(())
}

fn write_json_report(path: &Path, test_results: &[TestResult], total_elapsed: Duration, seed: u64) {
    let passed = test_results
        .iter()
        .filter(|r| r.status == TestResultStatus::Passed)
        .count();
    let failed = test_results
        .iter()
        .filter(|r| r.status == TestResultStatus::Failed)
        .count();
    let panicked = test_results
        .iter()
        .filter(|r| r.status == TestResultStatus::Panicked)
        .count();
    let cumulative_secs: f64 = test_results.iter().map(|r| r.duration.as_secs_f64()).sum();
    let wall_secs = total_elapsed.as_secs_f64();
    let effective_concurrency = if wall_secs > 0.0 {
        cumulative_secs / wall_secs
    } else {
        0.0
    };

    let specs_json: Vec<serde_json::Value> = test_results
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": r.spec_name,
                "base_name": r.base_name,
                "status": format!("{:?}", r.status),
                "duration_secs": r.duration.as_secs_f64(),
                "detail": r.detail,
                "spec_dir": r.spec_dir.as_ref().map(|p| p.display().to_string()),
                "report_path": r.report_path.as_ref().map(|p| p.display().to_string()),
                "runner_log_path": r.runner_log_path.as_ref().map(|p| p.display().to_string()),
                "node_log_paths": r
                    .node_log_paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>(),
            })
        })
        .collect();

    let payload = serde_json::json!({
        "seed": seed,
        "total": test_results.len(),
        "passed": passed,
        "failed": failed,
        "panicked": panicked,
        "wall_secs": wall_secs,
        "cumulative_secs": cumulative_secs,
        "effective_concurrency": effective_concurrency,
        "specs": specs_json,
    });

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(err) = fs::create_dir_all(parent)
    {
        eprintln!(
            "Failed to create directory for JSON report {}: {}",
            path.display(),
            err
        );
        return;
    }

    match serde_json::to_string_pretty(&payload) {
        Ok(content) => {
            if let Err(err) = fs::write(path, content) {
                eprintln!("Failed to write JSON report {}: {}", path.display(), err);
            } else {
                println!("JSON report: {}", path.display());
            }
        }
        Err(err) => eprintln!("Failed to serialize JSON report: {err}"),
    }
}

fn print_failures(test_results: &[TestResult]) {
    let failures: Vec<_> = test_results
        .iter()
        .filter(|r| r.status != TestResultStatus::Passed)
        .collect();
    if failures.is_empty() {
        return;
    }

    println!("\nFailures ({}):", failures.len());
    for result in failures {
        let status = match result.status {
            TestResultStatus::Passed => "PASS",
            TestResultStatus::Failed => "FAIL",
            TestResultStatus::Panicked => "PANIC",
        };
        let detail = result.detail.as_deref().unwrap_or("unknown failure");
        println!(
            "  - [{status}] {} ({}): {}",
            result.spec_name,
            format_duration(result.duration),
            detail
        );
        if let Some(report_path) = &result.report_path
            && report_path.exists()
        {
            println!("      report: {}", report_path.display());
        }
        if let Some(runner_log_path) = &result.runner_log_path
            && runner_log_path.exists()
        {
            println!("      runner: {}", runner_log_path.display());
        }
        if let Some(spec_dir) = &result.spec_dir
            && spec_dir.exists()
        {
            println!("      dir:    {}", spec_dir.display());
        }
        for log_path in &result.node_log_paths {
            if log_path.exists() {
                println!("      log:    {}", log_path.display());
            }
        }
    }
}

fn print_results(
    test_results: &mut [TestResult],
    total_elapsed: Duration,
    seed: u64,
    repeat: usize,
) {
    let rule = "-".repeat(89);
    println!("\n{rule}");
    println!("{:65} | {:8} | {:>10}", "TEST", "STATUS", "DURATION");
    println!("{rule}");

    test_results.sort_by(|a, b| (&a.status, a.duration).cmp(&(&b.status, b.duration)));

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut panicked = 0usize;
    let mut cumulative = Duration::ZERO;
    for result in test_results.iter() {
        match result.status {
            TestResultStatus::Passed => passed += 1,
            TestResultStatus::Failed => failed += 1,
            TestResultStatus::Panicked => panicked += 1,
        }
        cumulative += result.duration;
        println!(
            "{:65} | {:8} | {:>10}",
            result.spec_name,
            format!("{:?}", result.status),
            format_duration(result.duration),
        );
    }

    let total = test_results.len();
    let effective_parallelism = if total_elapsed.as_secs_f64() > 0.0 {
        cumulative.as_secs_f64() / total_elapsed.as_secs_f64()
    } else {
        0.0
    };

    println!("{rule}");
    println!(
        "Summary: {passed} passed, {failed} failed, {panicked} panicked ({total} total, seed={seed})"
    );
    println!(
        "Elapsed: {} wall | {} cumulative ({effective_parallelism:.2}x effective concurrency)",
        format_duration(total_elapsed),
        format_duration(cumulative),
    );

    if repeat > 1 {
        let mut by_spec: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for result in test_results.iter() {
            let entry = by_spec.entry(result.base_name.as_str()).or_insert((0, 0));
            if result.status == TestResultStatus::Passed {
                entry.0 += 1;
            }
            entry.1 += 1;
        }
        let mut rows: Vec<(&str, usize, usize)> = by_spec
            .into_iter()
            .map(|(name, (ok, n))| (name, ok, n))
            .collect();
        // Sort lowest pass rate first so flaky specs appear at the top.
        rows.sort_by(|a, b| {
            let rate_a = a.1 * b.2;
            let rate_b = b.1 * a.2;
            rate_a.cmp(&rate_b).then_with(|| a.0.cmp(b.0))
        });
        println!("\nRepeat pass rate ({repeat} runs per spec):");
        for (name, ok, n) in rows {
            let pct = if n > 0 {
                (ok as f64 / n as f64) * 100.0
            } else {
                0.0
            };
            println!("  {ok:>3}/{n:<3} ({pct:5.1}%)  {name}");
        }
    }

    if total > 1 {
        let mut by_duration: Vec<&TestResult> = test_results.iter().collect();
        by_duration.sort_by_key(|b| Reverse(b.duration));
        let top_n = min(10, by_duration.len());
        println!("\nSlowest {top_n} spec(s):");
        for result in by_duration.into_iter().take(top_n) {
            println!(
                "  {:>8}  {} ({:?})",
                format_duration(result.duration),
                result.spec_name,
                result.status
            );
        }
    }

    print_failures(test_results);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_ranges() {
        assert_eq!(format_duration(Duration::from_millis(420)), "0.42s");
        assert_eq!(format_duration(Duration::from_millis(9_950)), "9.95s");
        assert_eq!(format_duration(Duration::from_millis(12_340)), "12.3s");
        assert_eq!(format_duration(Duration::from_millis(92_400)), "1m 32.4s");
    }

    #[test]
    fn extract_panic_message_collapses_lines() {
        let payload: Box<dyn Any + Send> = Box::new("line1\n\n  line2  ".to_string());
        assert_eq!(extract_panic_message(payload.as_ref()), "line1 | line2");
        let str_payload: Box<dyn Any + Send> = Box::new("boom");
        assert_eq!(extract_panic_message(str_payload.as_ref()), "boom");
    }

    #[test]
    fn spec_matches_filter_is_case_insensitive() {
        assert!(spec_matches_filter("CompactBlockEmpty", "compact"));
        assert!(spec_matches_filter("CompactBlockEmpty", "BLOCK"));
        assert!(!spec_matches_filter("CompactBlockEmpty", "rbf"));
    }

    #[test]
    fn parse_shard_valid_and_invalid() {
        assert_eq!(parse_shard("0/8"), Ok((0, 8)));
        assert_eq!(parse_shard("7/8"), Ok((7, 8)));
        assert_eq!(parse_shard("0/1"), Ok((0, 1)));
        assert!(parse_shard("8/8").is_err());
        assert!(parse_shard("0/0").is_err());
        assert!(parse_shard("abc").is_err());
        assert!(parse_shard("-1/4").is_err());
    }

    #[test]
    fn lpt_shard_partition_is_disjoint_complete_and_balanced() {
        let specs = all_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name()).collect();
        let num_shards = 8;

        let mut seen = HashSet::new();
        let mut shard_weights = Vec::with_capacity(num_shards);
        let mut sequential_shards = HashSet::new();

        for shard_idx in 0..num_shards {
            let bucket = lpt_shard_indices(&names, shard_idx, num_shards);
            let mut weight = 0u64;
            for idx in bucket {
                assert!(
                    seen.insert(idx),
                    "spec {} assigned to multiple shards",
                    names[idx]
                );
                weight += u64::from(spec_shard_weight(names[idx]));
                if SEQUENCIAL_TASKS.contains(&names[idx]) {
                    sequential_shards.insert(shard_idx);
                }
            }
            shard_weights.push(weight);
        }

        assert_eq!(
            seen.len(),
            names.len(),
            "every spec must be assigned to a shard"
        );
        assert_eq!(
            sequential_shards.len(),
            SEQUENCIAL_TASKS.len(),
            "sequential specs should land in distinct shards"
        );
        let min_w = *shard_weights.iter().min().unwrap();
        let max_w = *shard_weights.iter().max().unwrap();
        assert!(
            max_w - min_w <= 10,
            "shards should be tightly weight-balanced, got weights: {shard_weights:?}"
        );
    }

    #[test]
    fn next_coordinator_poll_timeout_tracks_nearest_deadline() {
        let base = Instant::now();
        let running = vec![RunningSpec {
            spec_name: "SlowSpec".to_string(),
            base_name: "SlowSpec".to_string(),
            started_at: base,
            spec_dir: PathBuf::from("/tmp/slow"),
        }];

        // No limits -> capped at 5s default.
        assert_eq!(
            next_coordinator_poll_timeout(base, base, 0, 0, &running),
            Duration::from_secs(5)
        );
        // Nearest spec_timeout of 1s overrides 5s cap.
        assert_eq!(
            next_coordinator_poll_timeout(base, base, 0, 1, &running),
            Duration::from_secs(1)
        );
        // Already past deadline -> 0ms so watchdog fires immediately.
        assert_eq!(
            next_coordinator_poll_timeout(base + Duration::from_secs(2), base, 0, 1, &running),
            Duration::ZERO
        );
        // max_time closer than spec_timeout wins.
        assert_eq!(
            next_coordinator_poll_timeout(base, base, 2, 10, &running),
            Duration::from_secs(2)
        );
    }
}
