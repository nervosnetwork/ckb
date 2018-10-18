use pow::{DummyPowEngine, PowEngine};
use std::sync::Arc;

pub fn dummy_pow_engine() -> Arc<dyn PowEngine> {
    Arc::new(DummyPowEngine::new())
}
