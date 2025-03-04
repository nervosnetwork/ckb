//! `Spawn` abstract async runtime, spawns a future onto the runtime

#![no_std]

use core::future::Future;

/// `Spawn` abstract async runtime, spawns a future onto the runtime
#[cfg(not(target_family = "wasm"))]
pub trait Spawn {
    /// This spawns the given future onto the runtime's executor
    fn spawn_task<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static;
}

#[cfg(target_family = "wasm")]
pub trait Spawn {
    /// This spawns the given future onto the runtime's executor
    fn spawn_task<F>(&self, task: F)
    where
        F: Future<Output = ()> + 'static;
}
