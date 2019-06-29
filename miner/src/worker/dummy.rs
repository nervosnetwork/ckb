use super::{Worker, WorkerMessage};
use ckb_core::header::Seal;
use ckb_logger::error;
use crossbeam_channel::{Receiver, Sender};
use indicatif::ProgressBar;
use numext_fixed_hash::H256;
use rand::{
    distributions::{self as dist, Distribution as _},
    random, thread_rng,
};
use serde_derive::{Deserialize, Serialize};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "delay_type")]
pub enum DummyConfig {
    Constant { value: u64 },
    Uniform { low: u64, high: u64 },
    Normal { mean: f64, std_dev: f64 },
    Poisson { lambda: f64 },
}

pub struct Dummy {
    delay: Delay,
    start: bool,
    pow_hash: Option<H256>,
    seal_tx: Sender<(H256, Seal)>,
    worker_rx: Receiver<WorkerMessage>,
}

pub enum Delay {
    Constant(u64),
    Uniform(dist::Uniform<u64>),
    Normal(dist::Normal),
    Poisson(dist::Poisson),
}

impl From<&DummyConfig> for Delay {
    fn from(config: &DummyConfig) -> Self {
        match config {
            DummyConfig::Constant { value } => Delay::Constant(*value),
            DummyConfig::Uniform { low, high } => Delay::Uniform(dist::Uniform::new(*low, *high)),
            DummyConfig::Normal { mean, std_dev } => {
                Delay::Normal(dist::Normal::new(*mean, *std_dev))
            }
            DummyConfig::Poisson { lambda } => Delay::Poisson(dist::Poisson::new(*lambda)),
        }
    }
}

impl Default for Delay {
    fn default() -> Self {
        Delay::Constant(5000)
    }
}

impl Delay {
    fn duration(&self) -> Duration {
        let mut rng = thread_rng();
        let millis = match self {
            Delay::Constant(v) => *v,
            Delay::Uniform(ref d) => d.sample(&mut rng),
            Delay::Normal(ref d) => d.sample(&mut rng) as u64,
            Delay::Poisson(ref d) => d.sample(&mut rng),
        };
        Duration::from_millis(millis)
    }
}

impl Dummy {
    pub fn new(
        config: &DummyConfig,
        seal_tx: Sender<(H256, Seal)>,
        worker_rx: Receiver<WorkerMessage>,
    ) -> Self {
        Self {
            start: true,
            pow_hash: None,
            delay: config.into(),
            seal_tx,
            worker_rx,
        }
    }

    fn poll_worker_message(&mut self) {
        if let Ok(msg) = self.worker_rx.recv() {
            match msg {
                WorkerMessage::NewWork(pow_hash) => self.pow_hash = Some(pow_hash),
                WorkerMessage::Stop => {
                    self.start = false;
                }
                WorkerMessage::Start => {
                    self.start = true;
                }
            }
        }
    }

    fn solve(&self, pow_hash: &H256, nonce: u64) {
        thread::sleep(self.delay.duration());
        let seal = Seal::new(nonce, Vec::new().into());
        if let Err(err) = self.seal_tx.send((pow_hash.clone(), seal)) {
            error!("seal_tx send error {:?}", err);
        }
    }
}

impl Worker for Dummy {
    fn run(&mut self, _progress_bar: ProgressBar) {
        let mut current = self.pow_hash.clone();
        loop {
            self.poll_worker_message();
            if current != self.pow_hash && self.start {
                if let Some(pow_hash) = &self.pow_hash {
                    self.solve(pow_hash, random());
                }
            }

            current = self.pow_hash.clone();
        }
    }
}
