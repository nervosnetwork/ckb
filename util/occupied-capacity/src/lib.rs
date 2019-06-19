//! Data structure measurement.

use proc_macro_hack::proc_macro_hack;

pub use occupied_capacity_core::{Capacity, Error, Ratio, Result};

#[proc_macro_hack]
pub use occupied_capacity_macros::capacity_bytes;
