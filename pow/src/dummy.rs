use super::PowEngine;
use ckb_core::header::{BlockNumber, Header};
use numext_fixed_uint::U256;

pub struct DummyPowEngine;

impl PowEngine for DummyPowEngine {
    fn verify_header(&self, _header: &Header) -> bool {
        true
    }

    fn verify_proof_difficulty(&self, _proof: &[u8], _difficulty: &U256) -> bool {
        true
    }

    fn verify(&self, _number: BlockNumber, _message: &[u8], _proof: &[u8]) -> bool {
        unreachable!()
    }

    fn proof_size(&self) -> usize {
        0
    }
}
