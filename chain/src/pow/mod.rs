use bigint::H256;
use byteorder::{ByteOrder, LittleEndian};
use core::difficulty::{boundary_to_difficulty, difficulty_to_boundary};
use core::header::{BlockNumber, Header, RawHeader, Seal};
use hash::blake2b;
use rand::{thread_rng, Rng};
use std::{thread, time};

mod clicker;
mod cuckoo;
mod ethash;

pub use self::clicker::Clicker;
pub use self::cuckoo::{Cuckoo, CuckooEngine};
pub use self::ethash::EthashEngine;

fn pow_message(pow_hash: &[u8], nonce: u64) -> [u8; 40] {
    let mut message = [0; 40];
    message[8..40].copy_from_slice(pow_hash);
    LittleEndian::write_u64(&mut message, nonce);
    message
}

pub trait PowEngine: Send + Sync {
    fn init(&self, number: BlockNumber);

    fn verify_header(&self, header: &Header) -> bool {
        let proof_hash: H256 = blake2b(&header.seal.proof).into();
        if boundary_to_difficulty(&proof_hash) < header.difficulty {
            return false;
        }

        let message = pow_message(&header.pow_hash()[..], header.seal.nonce);
        self.verify(header.number, &message, &header.seal.proof)
    }

    fn solve_header(&self, header: &RawHeader, nonce: u64) -> Option<Seal> {
        let message = pow_message(&header.pow_hash()[..], nonce);

        if let Some(proof) = self.solve(header.number, &message) {
            let result: H256 = blake2b(&proof).into();
            if result < difficulty_to_boundary(&header.difficulty) {
                return Some(Seal { nonce, proof });
            }
        }

        None
    }

    fn solve(&self, number: BlockNumber, message: &[u8]) -> Option<Vec<u8>>;

    fn verify(&self, number: BlockNumber, message: &[u8], proof: &[u8]) -> bool;
}

#[derive(Clone)]
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
        Some(Seal {
            nonce,
            proof: Vec::new(),
        })
    }

    fn verify(&self, _number: BlockNumber, _message: &[u8], _proof: &[u8]) -> bool {
        true
    }

    fn solve(&self, _number: BlockNumber, _message: &[u8]) -> Option<Vec<u8>> {
        Some(Vec::new())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use hash::blake2b;
    #[test]
    fn test_pow_message() {
        let zero_hash: H256 = blake2b(&[]).into();
        let nonce = u64::max_value();
        let message = pow_message(&zero_hash, nonce);
        assert_eq!(
            message.to_vec(),
            [
                255, 255, 255, 255, 255, 255, 255, 255, 14, 87, 81, 192, 38, 229, 67, 178, 232,
                171, 46, 176, 96, 153, 218, 161, 209, 229, 223, 71, 119, 143, 119, 135, 250, 171,
                69, 205, 241, 47, 227, 168
            ]
                .to_vec()
        );
    }
}
