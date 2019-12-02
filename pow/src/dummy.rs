use super::PowEngine;
use ckb_types::core::HeaderContext;

pub struct DummyPowEngine;

impl PowEngine for DummyPowEngine {
    fn verify(&self, _header_ctx: &HeaderContext) -> bool {
        true
    }
}
