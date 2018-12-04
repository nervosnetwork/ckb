use super::super::setup::Setup;
use channel::unbounded;
use ckb_miner::{Client, Miner, Shared};
use ckb_util::RwLock;
use std::sync::Arc;
use std::thread::Builder;

pub fn miner(setup: Setup) {
    let (new_job_tx, new_job_rx) = unbounded();
    let shared = Shared {
        inner: Arc::new(RwLock::new(None)),
    };

    let client = Client {
        shared: shared.clone(),
        new_job_tx,
        config: setup.configs.miner,
    };

    let miner = Miner {
        pow: setup.chain_spec.pow_engine(),
        new_job_rx,
        shared,
        client: client.clone(),
    };

    let thread_builder = Builder::new();

    thread_builder
        .spawn(move || client.run())
        .expect("Start client failed!");

    miner.run()
}
