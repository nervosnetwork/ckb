use ckb_app_config::{ExitCode, MinerArgs};
use ckb_miner::{Client, Miner};
use ckb_util::Mutex;
use crossbeam_channel::unbounded;
use std::sync::Arc;
use std::thread;

pub fn miner(args: MinerArgs) -> Result<(), ExitCode> {
    let (new_work_tx, new_work_rx) = unbounded();

    let work = Arc::new(Mutex::new(None));

    let client = Client::new(Arc::clone(&work), new_work_tx, args.config);

    let miner = Miner::new(work, args.pow_engine, new_work_rx, client.clone());

    thread::Builder::new()
        .name("client".to_string())
        .spawn(move || client.poll_block_template())
        .expect("Start client failed!");

    miner.run();
    Ok(())
}
