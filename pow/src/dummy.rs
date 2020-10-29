use super::PowEngine;
use ckb_types::packed::Header;

/// TODO(doc): @quake
pub struct DummyPowEngine;

impl PowEngine for DummyPowEngine {
    fn verify(&self, _header: &Header) -> bool {
        true
    }
}
