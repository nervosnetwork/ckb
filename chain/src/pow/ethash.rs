use super::PowEngine;
use core::header::BlockNumber;
use ethash::{get_epoch, Ethash};
use std::path::Path;

pub struct EthashEngine {
    ethash: Ethash,
}

impl EthashEngine {
    pub fn new<P: AsRef<Path>>(cache_path: P) -> Self {
        EthashEngine {
            ethash: Ethash::new(cache_path),
        }
    }
}

impl PowEngine for EthashEngine {
    fn init(&self, number: BlockNumber) {
        self.ethash.gen_dataset(get_epoch(number));
    }

    fn verify(&self, _number: BlockNumber, _message: &[u8], _proof: &[u8]) -> bool {
        // TODO need ethash lib refactoring
        // self.ethash.verify(number, message, proof)
        true
    }

    fn solve(&self, _number: BlockNumber, _message: &[u8]) -> Option<Vec<u8>> {
        // TODO need ethash lib refactoring
        // self.ethash.solve(number, message)
        None
    }
}
