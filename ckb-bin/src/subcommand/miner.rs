use ckb_app_config::{ExitCode, MinerArgs, MinerConfig};
use ckb_async_runtime::Handle;
use ckb_channel::unbounded;
use ckb_miner::{Client, Miner};
use ckb_network::{DefaultExitHandler, ExitHandler};
use std::thread;

pub fn miner(args: MinerArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let (new_work_tx, new_work_rx) = unbounded();
    let MinerConfig { client, workers } = args.config;
    let exit_handler = DefaultExitHandler::default();

    let client = Client::new(new_work_tx, client, async_handle);
    let (mut miner, miner_stop) = Miner::new(
        args.pow_engine,
        client.clone(),
        new_work_rx,
        &workers,
        args.limit,
    );

    ckb_memory_tracker::track_current_process_simple(args.memory_tracker.interval);

    let client_stop = client.spawn_background();

    thread::Builder::new()
        .name("client".to_string())
        .spawn(move || miner.run())
        .expect("Start client failed!");

    let exit_handler_clone = exit_handler.clone();
    ctrlc::set_handler(move || {
        exit_handler_clone.notify_exit();
    })
    .expect("Error setting Ctrl-C handler");
    exit_handler.wait_for_exit();

    drop(client_stop);
    drop(miner_stop);
    Ok(())
}
