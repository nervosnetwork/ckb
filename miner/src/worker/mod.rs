mod cuckoo_simple;
mod dummy;

use crate::config::{WorkerConfig, WorkerType};
use ckb_core::header::Seal;
use ckb_logger::error;
use ckb_pow::{CuckooEngine, DummyPowEngine, PowEngine};
use crossbeam_channel::{unbounded, Sender};
use cuckoo_simple::CuckooSimple;
use dummy::Dummy;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use numext_fixed_hash::H256;
use std::sync::Arc;
use std::thread;

#[derive(Clone)]
pub enum WorkerMessage {
    Stop,
    Start,
    NewWork(H256),
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

pub fn start_worker(
    pow: Arc<dyn PowEngine>,
    config: &WorkerConfig,
    seal_tx: Sender<(H256, Seal)>,
) -> WorkerController {
    let mp = MultiProgress::new();
    let spinner_style = ProgressStyle::default_bar()
        .template("{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] {wide_msg}");

    match config.worker_type {
        WorkerType::Dummy => {
            if let Some(_dummy_engine) = pow.as_any().downcast_ref::<DummyPowEngine>() {
                let worker_name = "Dummy-Worker";
                let pb = mp.add(ProgressBar::new(100));
                pb.set_style(spinner_style.clone());
                pb.set_prefix(&worker_name);

                let (worker_tx, worker_rx) = unbounded();
                let mut worker = Dummy::new(config, seal_tx, worker_rx);

                thread::Builder::new()
                    .name(worker_name.to_string())
                    .spawn(move || {
                        worker.run(pb);
                    })
                    .expect("Start `Dummy` worker thread failed");
                WorkerController::new(vec![worker_tx])
            } else {
                panic!("incompatible pow engine and worker type");
            }
        }
        WorkerType::CuckooSimple => {
            if let Some(cuckoo_engine) = pow.as_any().downcast_ref::<CuckooEngine>() {
                let threads: usize = config.get_value("threads", 1);

                let worker_txs = (0..threads)
                    .map(|i| {
                        let worker_name = format!("CuckooSimple-Worker-{}", i);
                        // `100` is the len of progress bar, we can use any dummy value here, since we only show the spinner in console.
                        // `17` is a prime number, it will draw each spinner char in different ticks.
                        let pb = mp.add(ProgressBar::new(100));
                        pb.set_draw_delta(17);
                        pb.set_style(spinner_style.clone());
                        pb.set_prefix(&worker_name);

                        let (worker_tx, worker_rx) = unbounded();
                        let (cuckoo, seal_tx) = (cuckoo_engine.cuckoo.clone(), seal_tx.clone());
                        thread::Builder::new()
                            .name(worker_name)
                            .spawn(move || {
                                let mut worker = CuckooSimple::new(cuckoo, seal_tx, worker_rx);
                                worker.run(pb);
                            })
                            .expect("Start `CuckooSimple` worker thread failed");
                        worker_tx
                    })
                    .collect();

                thread::spawn(move || {
                    mp.join().expect("MultiProgress join failed");
                });

                WorkerController::new(worker_txs)
            } else {
                panic!("incompatible pow engine and worker type");
            }
        }
    }
}

pub trait Worker {
    fn run(&mut self, progress_bar: ProgressBar);
}
