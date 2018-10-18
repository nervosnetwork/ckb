use super::PowEngine;
use ckb_core::header::{BlockNumber, Header, RawHeader, Seal};
use rand::{thread_rng, Rng};
use std::any::Any;
use std::{thread, time};

#[derive(Copy, Clone)]
pub struct DummyPowEngine {}

impl DummyPowEngine {
    pub fn new() -> Self {
        DummyPowEngine {}
    }
}
impl Default for DummyPowEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PowEngine for DummyPowEngine {
    fn init(&self, _number: BlockNumber) {}

    fn verify_header(&self, _header: &Header) -> bool {
        true
    }

    fn solve_header(&self, _header: &RawHeader, nonce: u64) -> Option<Seal> {
        // Sleep for some time before returning result to miner
        let seconds = thread_rng().gen_range(5, 20);
        let duration = time::Duration::from_secs(seconds);
        thread::sleep(duration);
        Some(Seal::new(nonce, vec![]))
    }

    fn verify(&self, _number: BlockNumber, _message: &[u8], _proof: &[u8]) -> bool {
        true
    }

    fn solve(&self, _number: BlockNumber, _message: &[u8]) -> Option<Vec<u8>> {
        Some(Vec::new())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
