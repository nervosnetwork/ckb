use super::PowEngine;
use ckb_core::header::{BlockNumber, Header, RawHeader, Seal};
use rand::{
    distributions::{self as dist, Distribution as _},
    thread_rng,
};
use serde_derive::{Deserialize, Serialize};
use std::{fmt, thread, time};

#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct DummyPowParams {
    // Delay offset (in milliseconds)
    #[serde(skip)]
    delay: Distribution,
}

impl fmt::Display for DummyPowParams {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "(delay: {:?})", self.delay)
    }
}

impl DummyPowParams {
    fn gen_delay(&self) -> DummyPowDelay {
        match self.delay {
            Distribution::Constant { value } => DummyPowDelay::Constant(value),
            Distribution::Uniform { low, high } => {
                DummyPowDelay::Uniform(dist::Uniform::new(low, high))
            }
            Distribution::Normal { mean, std_dev } => {
                DummyPowDelay::Normal(dist::Normal::new(mean as f64, std_dev as f64))
            }
            Distribution::Poisson { lambda } => {
                DummyPowDelay::Poisson(dist::Poisson::new(lambda as f64))
            }
        }
    }
}

// TODO Enhance: we can add more distributions to mock POW
// Ref: https://docs.rs/rand/latest/rand/distributions/index.html
#[derive(Deserialize, Copy, Clone, Eq, PartialEq, Hash, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Distribution {
    Constant { value: u64 },
    Uniform { low: u64, high: u64 },
    Normal { mean: u64, std_dev: u64 },
    Poisson { lambda: u64 },
}

impl Default for Distribution {
    fn default() -> Self {
        Distribution::Constant { value: 5000 }
    }
}

#[derive(Copy, Clone)]
pub enum DummyPowDelay {
    Constant(u64),
    Uniform(dist::Uniform<u64>),
    Normal(dist::Normal),
    Poisson(dist::Poisson),
}

impl DummyPowDelay {
    fn duration(&self) -> time::Duration {
        let mut rng = thread_rng();
        let millis = match self {
            DummyPowDelay::Constant(v) => *v,
            DummyPowDelay::Uniform(ref d) => d.sample(&mut rng),
            DummyPowDelay::Normal(ref d) => d.sample(&mut rng) as u64,
            DummyPowDelay::Poisson(ref d) => d.sample(&mut rng),
        };
        time::Duration::from_millis(millis)
    }
}

#[derive(Copy, Clone)]
pub struct DummyPowEngine {
    delay: DummyPowDelay,
}

impl DummyPowEngine {
    pub fn new(params: DummyPowParams) -> Self {
        let delay = params.gen_delay();
        DummyPowEngine { delay }
    }
}

impl Default for DummyPowEngine {
    fn default() -> Self {
        let params = DummyPowParams::default();
        Self::new(params)
    }
}

impl PowEngine for DummyPowEngine {
    fn init(&self, _number: BlockNumber) {}

    fn verify_header(&self, _header: &Header) -> bool {
        true
    }

    fn solve_header(&self, _header: &RawHeader, nonce: u64) -> Option<Seal> {
        // Sleep for some time before returning result to miner
        thread::sleep(self.delay.duration());
        Some(Seal::new(nonce, vec![]))
    }

    fn verify(&self, _number: BlockNumber, _message: &[u8], _proof: &[u8]) -> bool {
        true
    }

    fn solve(&self, _number: BlockNumber, _message: &[u8]) -> Option<Vec<u8>> {
        Some(Vec::new())
    }
}
