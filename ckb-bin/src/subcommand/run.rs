use crate::helper::deadlock_detection;
use ckb_app_config::{ExitCode, RunArgs};
use ckb_async_runtime::Handle;
use ckb_build_info::Version;
use ckb_launcher::Launcher;
use ckb_logger::info;
use ckb_stop_handler::{broadcast_exit_signals, wait_all_ckb_services_exit};

use ckb_types::core::cell::setup_system_cell_cache;

pub fn run(args: RunArgs, version: Version, async_handle: Handle) -> Result<(), ExitCode> {
    deadlock_detection();

    info!("ckb version: {}", version);

    let mut launcher = Launcher::new(args, version, async_handle);

    let block_assembler_config = launcher.sanitize_block_assembler_config()?;
    let miner_enable = block_assembler_config.is_some();

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

    launcher.check_assume_valid_target(&shared);

    let chain_controller = launcher.start_chain_service(&shared, pack.take_proposal_table());

    launcher.start_block_filter(&shared);

    let (network_controller, _rpc_server) = launcher.start_network_and_rpc(
        &shared,
        chain_controller.non_owning_clone(),
        miner_enable,
        pack.take_relay_tx_receiver(),
    );

    let tx_pool_builder = pack.take_tx_pool_builder();
    tx_pool_builder.start(network_controller.non_owning_clone());

    ctrlc::set_handler(|| {
        info!("Trapped exit signal, exiting...");
        broadcast_exit_signals();
    })
    .expect("Error setting Ctrl-C handler");

    wait_all_ckb_services_exit();

    Ok(())
}
