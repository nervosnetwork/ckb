use byteorder::{ByteOrder, LittleEndian};
use ckb_core::difficulty::difficulty_to_target;
use ckb_core::header::{BlockNumber, Header, Seal};
use ckb_core::Bytes;
use hash::blake2b_256;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

mod cuckoo;
mod dummy;

pub use crate::cuckoo::{Cuckoo, CuckooEngine, CuckooParams};
pub use crate::dummy::{DummyPowEngine, DummyPowParams};

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
#[serde(tag = "func", content = "params")]
pub enum Pow {
    Dummy(DummyPowParams),
    Cuckoo(CuckooParams),
}

impl fmt::Display for Pow {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Pow::Dummy(params) => write!(f, "Dummy{}", params),
            Pow::Cuckoo(params) => write!(f, "Cuckoo{}", params),
        }
    }
}

impl Pow {
    pub fn engine(&self) -> Arc<dyn PowEngine> {
        match *self {
            Pow::Dummy(params) => Arc::new(DummyPowEngine::new(params)),
            Pow::Cuckoo(params) => Arc::new(CuckooEngine::new(params)),
        }
    }
}

fn pow_message(pow_hash: &H256, nonce: u64) -> [u8; 40] {
    let mut message = [0; 40];
    message[8..40].copy_from_slice(&pow_hash[..]);
    LittleEndian::write_u64(&mut message, nonce);
    message
}

pub struct Work {
    pub block_number: BlockNumber,
    pub pow_hash: H256,
    pub target_difficulty: H256,
    pub nonce: u64,
}

pub trait PowEngine: Send + Sync {
    fn init(&self, number: BlockNumber);

    fn verify_header(&self, header: &Header) -> bool {
        let proof_hash: H256 = blake2b_256(&header.proof()).into();
        if proof_hash >= difficulty_to_target(&header.difficulty()) {
            return false;
        }

        let message = pow_message(&header.pow_hash(), header.nonce());
        self.verify(header.number(), &message, &header.proof())
    }

    fn find_seal(&self, work: &Work) -> Option<Seal> {
        let message = pow_message(&work.pow_hash, work.nonce);

        if let Some(proof) = self.solve(work.block_number, &message) {
            let result: H256 = blake2b_256(&proof).into();
            if result < work.target_difficulty {
                return Some(Seal::new(work.nonce, Bytes::from(proof)));
            }
        }

        None
    }

    fn solve(&self, number: BlockNumber, message: &[u8]) -> Option<Vec<u8>>;

    fn verify(&self, number: BlockNumber, message: &[u8], proof: &[u8]) -> bool;

    fn proof_size(&self) -> usize;
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
