use ckb_app_config::{exit_failure, ExitCode, MinerArgs, MinerConfig};
use ckb_channel::unbounded;
use ckb_miner::{Client, Miner};
use std::thread;

pub fn miner(args: MinerArgs) -> Result<(), ExitCode> {
    let (new_work_tx, new_work_rx) = unbounded();
    let MinerConfig { client, workers } = args.config;

    let mut client = Client::new(new_work_tx, client);
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
        .map_err(|_| exit_failure!("Start client failed!"))?;

    miner.run();
    Ok(())
}
