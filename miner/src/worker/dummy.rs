use super::{Worker, WorkerMessage};
use crate::Work;
use ckb_app_config::DummyConfig;
use ckb_channel::{Receiver, Sender};
use ckb_logger::error;
use ckb_types::packed::Byte32;
use indicatif::ProgressBar;
use rand::thread_rng;
use rand_distr::{self as dist, Distribution as _};
use std::thread;
use std::time::{Duration, Instant};

pub struct Dummy {
    delay: Delay,
    start: bool,
    pow_work: Option<(Byte32, Work)>,
    nonce_tx: Sender<(Byte32, Work, u128)>,
    worker_rx: Receiver<WorkerMessage>,
}

pub enum Delay {
    Constant(u64),
    Uniform(dist::Uniform<u64>),
    Normal(dist::Normal<f64>),
    Poisson(dist::Poisson<f64>),
}

impl TryFrom<&DummyConfig> for Delay {
    type Error = Box<dyn std::error::Error>;

    fn try_from(config: &DummyConfig) -> Result<Self, Self::Error> {
        match config {
            DummyConfig::Constant { value } => Ok(Delay::Constant(*value)),
            DummyConfig::Uniform { low, high } => {
                Ok(Delay::Uniform(dist::Uniform::new(*low, *high)))
            }
            DummyConfig::Normal { mean, std_dev } => dist::Normal::new(*mean, *std_dev)
                .map(Delay::Normal)
                .map_err(Into::into),
            DummyConfig::Poisson { lambda } => dist::Poisson::new(*lambda)
                .map(Delay::Poisson)
                .map_err(Into::into),
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
            Delay::Uniform(d) => d.sample(&mut rng),
            Delay::Normal(d) => d.sample(&mut rng) as u64,
            Delay::Poisson(d) => d.sample(&mut rng) as u64,
        };
        Duration::from_millis(millis)
    }
}

impl Dummy {
    pub fn try_new(
        config: &DummyConfig,
        nonce_tx: Sender<(Byte32, Work, u128)>,
        worker_rx: Receiver<WorkerMessage>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Delay::try_from(config).map(|delay| Self {
            start: true,
            pow_work: None,
            delay,
            nonce_tx,
            worker_rx,
        })
    }

    fn poll_worker_message(&mut self) {
        while let Ok(msg) = self.worker_rx.try_recv() {
            match msg {
                WorkerMessage::NewWork { pow_hash, work, .. } => {
                    self.pow_work = Some((pow_hash, work))
                }
                WorkerMessage::Stop => {
                    self.start = false;
                }
                WorkerMessage::Start => {
                    self.start = true;
                }
            }
        }
    }

    fn solve(&mut self, mut pow_hash: Byte32, mut work: Work, nonce: u128) {
        let instant = Instant::now();
        let delay = self.delay.duration();
        loop {
            thread::sleep(Duration::from_millis(10));
            if instant.elapsed() > delay {
                if let Err(err) = self.nonce_tx.send((pow_hash, work, nonce)) {
                    error!("nonce_tx send error {:?}", err);
                }
                return;
            }
            // if there is new work and pow_hash changed, start working on the new one
            if let Ok(WorkerMessage::NewWork {
                pow_hash: new_pow_hash,
                work: new_work,
                ..
            }) = self.worker_rx.try_recv()
            {
                if new_pow_hash != pow_hash {
                    pow_hash = new_pow_hash;
                    work = new_work;
                }
            }
        }
    }
}

impl Worker for Dummy {
    fn run<G: FnMut() -> u128>(&mut self, mut rng: G, _progress_bar: ProgressBar) {
        loop {
            self.poll_worker_message();
            if self.start {
                if let Some((pow_hash, work)) = self.pow_work.clone() {
                    self.solve(pow_hash, work, rng());
                }
            }
        }
    }
}
