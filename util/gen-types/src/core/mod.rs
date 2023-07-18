mod types;
#[cfg(feature = "calc-hash")]
mod view;

pub use types::*;

#[cfg(feature = "calc-hash")]
pub use view::*;

pub use ckb_occupied_capacity::{
    capacity_bytes, Capacity, Error as CapacityError, Ratio, Result as CapacityResult,
};

/// Block number.
pub type BlockNumber = u64;

/// Epoch number.
pub type EpochNumber = u64;

/// Cycle number.
pub type Cycle = u64;

/// Version number.
pub type Version = u32;
