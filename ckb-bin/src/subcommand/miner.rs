use ckb_app_config::{ExitCode, MinerArgs, MinerConfig};
use ckb_async_runtime::Handle;
use ckb_channel::unbounded;
use ckb_miner::{Client, Miner};
use std::thread;

pub fn miner(args: MinerArgs, async_handle: Handle) -> Result<(), ExitCode> {
    let (new_work_tx, new_work_rx) = unbounded();
    let MinerConfig { client, workers } = args.config;

    let mut client = Client::new(new_work_tx, client, async_handle);
    let mut miner = Miner::new(
        args.pow_engine,
        client.clone(),
        new_work_rx,
        &workers,
        args.limit,
    );

    ckb_memory_tracker::track_current_process_simple(args.memory_tracker.interval);

    thread::Builder::new()
        .name("client".to_string())
        .spawn(move || client.poll_block_template())
        .expect("Start client failed!");

    miner.run();
    Ok(())
}
