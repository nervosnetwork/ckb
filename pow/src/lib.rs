use byteorder::{ByteOrder, LittleEndian};
use ckb_core::difficulty::difficulty_to_target;
use ckb_core::header::{BlockNumber, Header};
use hash::blake2b_256;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};
use std::any::Any;
use std::fmt;
use std::sync::Arc;

mod cuckoo;
mod dummy;

pub use crate::cuckoo::{Cuckoo, CuckooEngine, CuckooParams, CuckooSip};
pub use crate::dummy::DummyPowEngine;

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
#[serde(tag = "func", content = "params")]
pub enum Pow {
    Dummy,
    Cuckoo(CuckooParams),
}

impl fmt::Display for Pow {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Pow::Dummy => write!(f, "Dummy"),
            Pow::Cuckoo(params) => write!(f, "Cuckoo{}", params),
        }
    }
}

impl Pow {
    pub fn engine(&self) -> Arc<dyn PowEngine> {
        match *self {
            Pow::Dummy => Arc::new(DummyPowEngine),
            Pow::Cuckoo(params) => Arc::new(CuckooEngine::new(params)),
        }
    }
}

pub fn pow_message(pow_hash: &H256, nonce: u64) -> [u8; 40] {
    let mut message = [0; 40];
    message[8..40].copy_from_slice(&pow_hash[..]);
    LittleEndian::write_u64(&mut message, nonce);
    message
}

pub trait PowEngine: Send + Sync + AsAny {
    fn verify_header(&self, header: &Header) -> bool {
        if self.verify_proof_difficulty(&header.proof(), &header.difficulty()) {
            let message = pow_message(&header.pow_hash(), header.nonce());
            self.verify(header.number(), &message, &header.proof())
        } else {
            false
        }
    }

    fn verify_proof_difficulty(&self, proof: &[u8], difficulty: &U256) -> bool {
        let proof_hash: H256 = blake2b_256(proof).into();
        proof_hash < difficulty_to_target(difficulty)
    }

    fn verify(&self, number: BlockNumber, message: &[u8], proof: &[u8]) -> bool;

    fn proof_size(&self) -> usize;
}

pub trait AsAny {
    fn as_any(&self) -> &Any;
}

impl<T: Any> AsAny for T {
    fn as_any(&self) -> &Any {
        self
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use hash::blake2b_256;
    #[test]
    fn test_pow_message() {
        let zero_hash: H256 = blake2b_256(&[]).into();
        let nonce = u64::max_value();
        let message = pow_message(&zero_hash, nonce);
        assert_eq!(
            message.to_vec(),
            [
                255, 255, 255, 255, 255, 255, 255, 255, 68, 244, 198, 151, 68, 213, 248, 197, 93,
                100, 32, 98, 148, 157, 202, 228, 155, 196, 231, 239, 67, 211, 136, 197, 161, 47,
                66, 181, 99, 61, 22, 62
            ]
            .to_vec()
        );
    }
}
