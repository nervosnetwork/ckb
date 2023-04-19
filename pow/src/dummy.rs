use super::PowEngine;
use ckb_types::packed::Header;

/// A mock Pow Engine, mostly for development&test purpose, and may not used in real world verification
pub struct DummyPowEngine;

impl PowEngine for DummyPowEngine {
    /// This result will always be true
    fn verify(&self, _header: &Header) -> bool {
        true
    }
}
