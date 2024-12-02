//! Utilities for tokio runtime.

pub use tokio;
pub use tokio::runtime::Runtime;

#[cfg(not(target_family = "wasm"))]
pub use native::*;

#[cfg(target_family = "wasm")]
pub use browser::*;

#[cfg(target_family = "wasm")]
mod browser;
#[cfg(not(target_family = "wasm"))]
mod native;
