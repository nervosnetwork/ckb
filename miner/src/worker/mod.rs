mod dummy;
mod eaglesong_simple;

use crate::config::WorkerConfig;
use ckb_logger::error;
use ckb_pow::{DummyPowEngine, EaglesongPowEngine, PowEngine, TestnetPowEngine};
use ckb_types::{packed::Byte32, U256};
use crossbeam_channel::{unbounded, Sender};
use dummy::Dummy;
use eaglesong_simple::EaglesongSimple;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rand::{random, Rng};
use std::ops::Range;
use std::sync::Arc;
use std::thread;

pub use dummy::DummyConfig;
pub use eaglesong_simple::EaglesongSimpleConfig;

#[derive(Clone)]
pub enum WorkerMessage {
    Stop,
    Start,
    NewWork { pow_hash: Byte32, target: U256 },
}

pub struct WorkerController {
    inner: Vec<Sender<WorkerMessage>>,
}

impl WorkerController {
    pub fn new(inner: Vec<Sender<WorkerMessage>>) -> Self {
        Self { inner }
    }

    pub fn send_message(&self, message: WorkerMessage) {
        for worker_tx in self.inner.iter() {
            if let Err(err) = worker_tx.send(message.clone()) {
                error!("worker_tx send error {:?}", err);
            };
        }
    }
}

fn partition_nonce(id: u128, total: u128) -> Range<u128> {
    let span = u128::max_value() / total;
    let start = span * id;
    let end = match id {
        x if x < total - 1 => start + span,
        x if x == total - 1 => u128::max_value(),
        _ => unreachable!(),
    };
    Range { start, end }
}

fn nonce_generator(range: Range<u128>) -> impl FnMut() -> u128 {
    let mut rng = rand::thread_rng();
    let Range { start, end } = range;
    move || rng.gen_range(start, end)
}

const PROGRESS_BAR_TEMPLATE: &str = "{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] {msg}";

pub fn start_worker(
    pow: Arc<dyn PowEngine>,
    config: &WorkerConfig,
    nonce_tx: Sender<(Byte32, u128)>,
    mp: &MultiProgress,
) -> WorkerController {
    match config {
        WorkerConfig::Dummy(config) => {
            if pow.as_any().downcast_ref::<DummyPowEngine>().is_some() {
                let worker_name = "Dummy-Worker";
                let pb = mp.add(ProgressBar::new(100));
                pb.set_style(ProgressStyle::default_bar().template(PROGRESS_BAR_TEMPLATE));
                pb.set_prefix(&worker_name);

                let (worker_tx, worker_rx) = unbounded();
                let mut worker = Dummy::new(config, nonce_tx, worker_rx);

                thread::Builder::new()
                    .name(worker_name.to_string())
                    .spawn(move || {
                        let rng = || random();
                        worker.run(rng, pb);
                    })
                    .expect("Start `Dummy` worker thread failed");
                WorkerController::new(vec![worker_tx])
            } else {
                panic!("incompatible pow engine and worker type");
            }
        }
        WorkerConfig::EaglesongSimple(config) => {
            let is_testnet = config.is_testnet;
            if pow.as_any().downcast_ref::<EaglesongPowEngine>().is_some()
                || pow.as_any().downcast_ref::<TestnetPowEngine>().is_some()
            {
                let worker_txs = (0..config.threads)
                    .map(|i| {
                        let worker_name = if is_testnet {
                            format!("EaglesongSimple-TestnetWorker-{}", i)
                        } else {
                            format!("EaglesongSimple-Worker-{}", i)
                        };
                        let nonce_range = partition_nonce(i as u128, config.threads as u128);
                        // `100` is the len of progress bar, we can use any dummy value here,
                        // since we only show the spinner in console.
                        let pb = mp.add(ProgressBar::new(100));
                        pb.set_style(ProgressStyle::default_bar().template(PROGRESS_BAR_TEMPLATE));
                        pb.set_prefix(&worker_name);

                        let (worker_tx, worker_rx) = unbounded();
                        let nonce_tx = nonce_tx.clone();
                        thread::Builder::new()
                            .name(worker_name)
                            .spawn(move || {
                                let mut worker =
                                    EaglesongSimple::new(nonce_tx, worker_rx, is_testnet);
                                let rng = nonce_generator(nonce_range);
                                worker.run(rng, pb);
                            })
                            .expect("Start `EaglesongSimple` worker thread failed");
                        worker_tx
                    })
                    .collect();

                WorkerController::new(worker_txs)
            } else {
                panic!("incompatible pow engine and worker type");
            }
        }
    }
}

pub trait Worker {
    fn run<G: FnMut() -> u128>(&mut self, rng: G, progress_bar: ProgressBar);
}
