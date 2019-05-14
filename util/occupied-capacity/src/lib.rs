//! Data structure measurement.

use proc_macro_hack::proc_macro_hack;

pub use occupied_capacity_core::{Capacity, Error, OccupiedCapacity, Result};
pub use occupied_capacity_macros::HasOccupiedCapacity;

#[proc_macro_hack]
pub use occupied_capacity_macros::capacity_bytes;
