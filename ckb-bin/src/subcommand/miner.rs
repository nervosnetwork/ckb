use ckb_app_config::{ExitCode, MinerArgs, MinerConfig};
use ckb_miner::{Client, Miner};
use crossbeam_channel::unbounded;
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

    ckb_memory_tracker::track_current_process(args.memory_tracker.interval);

    thread::Builder::new()
        .name("client".to_string())
        .spawn(move || client.poll_block_template())
        .expect("Start client failed!");

    miner.run();
    Ok(())
}
