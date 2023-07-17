#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

pub mod prelude;

#[cfg(feature = "std")]
pub use ckb_fixed_hash::{h160, h256, H160, H256};
#[cfg(feature = "std")]
pub use numext_fixed_uint::{u256, U128, U256};

mod conversion;
pub mod core;
pub mod extension;
mod generated;
pub use generated::packed;
pub mod util;

//re-exports
pub use molecule::bytes;

cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        use std::vec;
    } else {
        use alloc::vec;
    }
}
