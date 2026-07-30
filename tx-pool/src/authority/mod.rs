//! Test-only construction of the frozen unified authority design.
//!
//! Until the atomic P9.7g cutover this module is compiled only for tests and
//! cannot participate in production ownership or decisions. It deliberately
//! uses project transaction identities and Rust ownership so the target API is
//! proven before any runtime path is switched.

mod plan;
mod resources;
mod state;
mod work;

#[cfg(test)]
mod tests;
