use ckb_app_config::{ExitCode, MinerArgs, MinerConfig};
use ckb_async_runtime::Handle;
use ckb_channel::unbounded;
use ckb_miner::{Client, Miner};
use ckb_stop_handler::{new_crossbeam_exit_rx, register_thread, wait_all_ckb_services_exit};
use std::thread;

pub fn miner(args: MinerArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let (new_work_tx, new_work_rx) = unbounded();
    let MinerConfig { client, workers } = args.config;

    let client = Client::new(new_work_tx, client, async_handle);
    let mut miner = Miner::new(
        args.pow_engine,
        client.clone(),
        new_work_rx,
        &workers,
        args.limit,
    );

    ckb_memory_tracker::track_current_process_simple(args.memory_tracker.interval);

    client.spawn_background();

    let stop_rx = new_crossbeam_exit_rx();
    const THREAD_NAME: &str = "client";
    let miner_jh = thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || miner.run(stop_rx))
        .expect("Start client failed!");
    register_thread(THREAD_NAME, miner_jh);

    wait_all_ckb_services_exit();

    Ok(())
}
