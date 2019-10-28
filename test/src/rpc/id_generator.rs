use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub struct IdGenerator {
    state: AtomicU64,
}

impl Default for IdGenerator {
    fn default() -> Self {
        IdGenerator {
            state: AtomicU64::new(1),
        }
    }
}

impl IdGenerator {
    pub fn new() -> IdGenerator {
        IdGenerator::default()
    }

    pub fn next(&self) -> u64 {
        self.state.fetch_add(1, Ordering::SeqCst)
    }
}
