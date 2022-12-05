mod main_node;
mod mock_node;
mod mock_sync;
mod modify_config;
mod run;

use ckb_logger::error;
use clap::Parser;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct BenchApp {
    /// The full path of ckb binary file for benchmark
    #[arg(long)]
    main_node_binary_path: PathBuf,

    /// The log filter for benchmark node, example: "info" or "info,ckb-sync=debug"
    #[arg(long, default_value = "info")]
    main_node_log_filter: String,

    /// The full path of an already synced mainnet node's data/db dir, the mock nodes will read from this dir.
    #[arg(long)]
    shared_ckb_db_path: PathBuf,

    /// Where the mock nodes and main node should put their configuration files and data file.
    #[arg(long)]
    work_dir: PathBuf,

    /// The benchmark program will stop when the main node reach `target_height`
    #[arg(long)]
    target_height: u64,

    /// The main node's rpc port, default is 18100.
    /// The main node's p2p port will always be its `rpc_port + 1`
    #[arg(long, default_value = "18100")]
    main_node_rpc_port: u64,

    /// The mock node's rpc port, default is 18102.
    /// The mock node's p2p port will always be its `rpc_port + 1`
    #[arg(long, default_value = "18100")]
    mock_node_rpc_port: u64,

    /// How many mock nodes will be started, default is 8.
    #[arg(long, default_value = "8")]
    mock_nodes_count: u64,
}

fn main() {
    let _log_guard = ckb_logger_service::init(
        None,
        ckb_logger_config::Config {
            filter: Some("info".to_string()),
            color: true,
            file: Default::default(),
            log_dir: Default::default(),
            log_to_file: false,
            log_to_stdout: true,
            emit_sentry_breadcrumbs: None,
            extra: Default::default(),
        },
    )
    .unwrap();

    let cli = BenchApp::parse();

    if !cli.main_node_binary_path.exists() {
        error!(
            "main node not exist on {}",
            cli.main_node_binary_path.display()
        );
        return;
    }

    if !cli.shared_ckb_db_path.exists() {
        error!(
            "shared ckb db not exist on {}",
            cli.shared_ckb_db_path.display()
        );
        return;
    }

    if cli.mock_nodes_count < 1 {
        error!("mock nodes count must be greater than 0");
        return;
    }

    let now = Instant::now();

    run::run(
        cli.main_node_binary_path,
        cli.main_node_log_filter,
        cli.shared_ckb_db_path,
        cli.work_dir,
        cli.mock_nodes_count,
        cli.target_height,
        cli.main_node_rpc_port,
        cli.mock_node_rpc_port,
    );
    println!("elapsed: {:?}", now.elapsed());
}
