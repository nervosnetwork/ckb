use super::{Worker, WorkerMessage};
use ckb_app_config::DummyConfig;
use ckb_channel::{Receiver, Sender};
use ckb_logger::error;
use ckb_types::packed::Byte32;
use indicatif::ProgressBar;
use rand::thread_rng;
use rand_distr::{self as dist, Distribution as _};
use std::convert::TryFrom;
use std::thread;
use std::time::Duration;

pub struct Dummy {
    delay: Delay,
    start: bool,
    pow_hash: Option<Byte32>,
    nonce_tx: Sender<(Byte32, u128)>,
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
            Delay::Uniform(ref d) => d.sample(&mut rng),
            Delay::Normal(ref d) => d.sample(&mut rng) as u64,
            Delay::Poisson(ref d) => d.sample(&mut rng) as u64,
        };
        Duration::from_millis(millis)
    }
}

impl Dummy {
    pub fn try_new(
        config: &DummyConfig,
        nonce_tx: Sender<(Byte32, u128)>,
        worker_rx: Receiver<WorkerMessage>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Delay::try_from(config).map(|delay| Self {
            start: true,
            pow_hash: None,
            delay,
            nonce_tx,
            worker_rx,
        })
    }

    fn poll_worker_message(&mut self) {
        if let Ok(msg) = self.worker_rx.recv() {
            match msg {
                WorkerMessage::NewWork { pow_hash, .. } => self.pow_hash = Some(pow_hash),
                WorkerMessage::Stop => {
                    self.start = false;
                }
                WorkerMessage::Start => {
                    self.start = true;
                }
            }
        }
    }

    fn solve(&self, pow_hash: &Byte32, nonce: u128) {
        thread::sleep(self.delay.duration());
        if let Err(err) = self.nonce_tx.send((pow_hash.clone(), nonce)) {
            error!("nonce_tx send error {:?}", err);
        }
    }
}

impl Worker for Dummy {
    fn run<G: FnMut() -> u128>(&mut self, mut rng: G, _progress_bar: ProgressBar) {
        let mut current = self.pow_hash.clone();
        loop {
            self.poll_worker_message();
            if current != self.pow_hash && self.start {
                if let Some(pow_hash) = &self.pow_hash {
                    self.solve(pow_hash, rng());
                }
            }

            current = self.pow_hash.clone();
        }
    }
}
