//! Collect constants used across ckb components.

/// hardfork constant
pub mod hardfork;
/// store constant
pub mod store;
/// sync constant
pub mod sync;

/// The maximum vm version number.
#[cfg(not(feature = "test-only"))]
pub const MAX_VM_VERSION: u8 = 1;

/// The fake maximum vm version number for test only.
///
/// This number should be 1 larger than the real maximum vm version number.
#[cfg(feature = "test-only")]
pub const MAX_VM_VERSION: u8 = 2;
