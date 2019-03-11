use byteorder::{ByteOrder, LittleEndian};
use ckb_core::difficulty::{boundary_to_difficulty, difficulty_to_boundary};
use ckb_core::header::{BlockNumber, Header, RawHeader, Seal};
use hash::blake2b;
use numext_fixed_hash::H256;
use serde_derive::Deserialize;
use std::any::Any;
use std::sync::Arc;

mod cuckoo;
mod dummy;

pub use crate::cuckoo::{Cuckoo, CuckooEngine, CuckooParams};
pub use crate::dummy::DummyPowEngine;

#[derive(Clone, Deserialize, Eq, PartialEq, Hash, Debug)]
#[serde(tag = "func", content = "params")]
pub enum Pow {
    Dummy,
    Cuckoo(CuckooParams),
}

impl Pow {
    pub fn engine(&self) -> Arc<dyn PowEngine> {
        match *self {
            Pow::Dummy => Arc::new(DummyPowEngine::new()),
            Pow::Cuckoo(params) => Arc::new(CuckooEngine::new(params)),
        }
    }
}

fn pow_message(pow_hash: &[u8], nonce: u64) -> [u8; 40] {
    let mut message = [0; 40];
    message[8..40].copy_from_slice(pow_hash);
    LittleEndian::write_u64(&mut message, nonce);
    message
}

pub trait PowEngine: Send + Sync {
    fn init(&self, number: BlockNumber);

    #[allow(clippy::op_ref)]
    fn verify_header(&self, header: &Header) -> bool {
        let proof_hash: H256 = blake2b(&header.proof()).into();
        if &boundary_to_difficulty(&proof_hash) < header.difficulty() {
            return false;
        }

        let message = pow_message(&header.pow_hash()[..], header.nonce());
        self.verify(header.number(), &message, &header.proof())
    }

    fn solve_header(&self, header: &RawHeader, nonce: u64) -> Option<Seal> {
        let message = pow_message(&header.pow_hash()[..], nonce);

        if let Some(proof) = self.solve(header.number(), &message) {
            let result: H256 = blake2b(&proof).into();
            if result < difficulty_to_boundary(&header.difficulty()) {
                return Some(Seal::new(nonce, proof));
            }
        }

        None
    }

    fn solve(&self, number: BlockNumber, message: &[u8]) -> Option<Vec<u8>>;

    fn verify(&self, number: BlockNumber, message: &[u8], proof: &[u8]) -> bool;
}

#[cfg(test)]
mod test {
    use super::*;
    use hash::blake2b;
    #[test]
    fn test_pow_message() {
        let zero_hash: H256 = blake2b(&[]).into();
        let nonce = u64::max_value();
        let message = pow_message(zero_hash.as_bytes(), nonce);
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
