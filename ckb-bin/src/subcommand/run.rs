use std::thread::available_parallelism;

use crate::helper::deadlock_detection;
use ckb_app_config::{ExitCode, RunArgs};
use ckb_async_runtime::{Handle, new_global_runtime};
use ckb_build_info::Version;
use ckb_launcher::Launcher;
use ckb_logger::info;
use ckb_logger::warn;
use ckb_resource::{Resource, TemplateContext};

use ckb_stop_handler::{broadcast_exit_signals, wait_all_ckb_services_exit};

use ckb_types::core::cell::setup_system_cell_cache;

pub fn run(args: RunArgs, version: Version, async_handle: Handle) -> Result<(), ExitCode> {
    check_default_db_options_exists(&args)?;
    deadlock_detection();

    let rpc_threads_num = calc_rpc_threads_num(&args);
    info!("ckb version: {}", version);
    info!("run rpc server with {} threads", rpc_threads_num);
    let (mut rpc_handle, _rpc_stop_rx, _runtime) = new_global_runtime(Some(rpc_threads_num));
    let launcher = Launcher::new(args, version, async_handle, rpc_handle.clone());

    let block_assembler_config = launcher.sanitize_block_assembler_config()?;
    let miner_enable = block_assembler_config.is_some();

    launcher.check_indexer_config()?;

    let (shared, mut pack) = launcher.build_shared(block_assembler_config)?;

    // spawn freezer background process
    let _freezer = shared.spawn_freeze();

    setup_system_cell_cache(
        shared.consensus().genesis_block(),
        shared.snapshot().as_ref(),
    )
    .expect("SYSTEM_CELL cache init once");

    rayon::ThreadPoolBuilder::new()
        .thread_name(|i| format!("RayonGlobal-{i}"))
        .build_global()
        .expect("Init the global thread pool for rayon failed");

    ckb_memory_tracker::track_current_process(
        launcher.args.config.memory_tracker.interval,
        Some(shared.store().db().inner()),
    );

    let chain_controller =
        launcher.start_chain_service(&shared, pack.take_chain_services_builder());

    launcher.start_block_filter(&shared);

    let network_controller = launcher.start_network_and_rpc(
        &shared,
        chain_controller,
        miner_enable,
        pack.take_relay_tx_receiver(),
    );

    let tx_pool_builder = pack.take_tx_pool_builder();
    tx_pool_builder.start(network_controller);

    info!("CKB service started ...");
    ctrlc::set_handler(|| {
        info!("Trapped exit signal, exiting...");
        broadcast_exit_signals();
    })
    .expect("Error setting Ctrl-C handler");

    rpc_handle.drop_guard();
    wait_all_ckb_services_exit();

    Ok(())
}

fn calc_rpc_threads_num(args: &RunArgs) -> usize {
    let system_parallelism: usize = available_parallelism().unwrap().into();
    let default_num = usize::max(system_parallelism, 1);
    args.config.rpc.threads.unwrap_or(default_num)
}

fn check_default_db_options_exists(args: &RunArgs) -> Result<(), ExitCode> {
    // check is there a default.db-options file exist in args.config.root_dir, if not, create one.
    let db_options_path = args.config.root_dir.join("default.db-options");

    // Check if the default.db-options file exists, if not, create one.
    if !db_options_path.exists() {
        warn!(
            "default.db-options file does not exist in {}, creating one.",
            args.config.root_dir.display()
        );
        // context_for_db_options is used to generate a default default.db-options file.
        let context_for_db_options = TemplateContext::new("", vec![]);

        // Attempt to export the bundled DB options to the specified path.
        Resource::bundled_db_options()
            .export(&context_for_db_options, &args.config.root_dir)
            .map_err(|_| ExitCode::Config)?;
    }
    Ok(())
}
