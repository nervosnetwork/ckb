use bigint::H256;
use byteorder::{ByteOrder, LittleEndian};
use core::difficulty::{boundary_to_difficulty, difficulty_to_boundary};
use core::header::{BlockNumber, Header, RawHeader, Seal};
use hash::blake2b;

mod cuckoo;
mod ethash;

pub use self::cuckoo::CuckooEngine;
pub use self::ethash::EthashEngine;

pub trait PowEngine: Send + Sync {
    fn init(&self, number: BlockNumber);

    fn verify_header(&self, header: &Header) -> bool {
        let proof_hash: H256 = blake2b(&header.seal.proof).into();
        if boundary_to_difficulty(&proof_hash) >= header.difficulty {
            return false;
        }

        let message = &mut [0; 40];
        header.pow_hash().copy_to(message);
        LittleEndian::write_u64(message, header.seal.nonce);

        self.verify(header.number, message, &header.seal.proof)
    }

    fn solve_header(&self, header: &RawHeader, nonce: u64) -> Option<Seal> {
        let message = &mut [0; 40];
        header.pow_hash().copy_to(message);
        LittleEndian::write_u64(message, nonce);

        if let Some(proof) = self.solve(header.number, message) {
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
