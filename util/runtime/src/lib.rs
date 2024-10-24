//! Utilities for tokio runtime.

pub use tokio;
pub use tokio::runtime::Runtime;

#[cfg(not(target_family = "wasm"))]
pub use native::*;

#[cfg(target_family = "wasm")]
pub use brower::*;

#[cfg(target_family = "wasm")]
mod brower;
#[cfg(not(target_family = "wasm"))]
mod native;
