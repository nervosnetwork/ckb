use super::super::setup::Setup;
use ckb_miner::{Client, Miner};
use ckb_util::RwLock;
use crossbeam_channel::unbounded;
use std::sync::Arc;
use std::thread;

pub fn miner(setup: Setup) {
    let (new_work_tx, new_work_rx) = unbounded();

    let work = Arc::new(RwLock::new(None));

    let client = Client::new(Arc::clone(&work), new_work_tx, setup.configs.miner);

    let miner = Miner::new(
        work,
        setup.chain_spec.pow_engine(),
        new_work_rx,
        client.clone(),
    );

    thread::Builder::new()
        .name("client".to_string())
        .spawn(move || client.run())
        .expect("Start client failed!");

    miner.run()
}
