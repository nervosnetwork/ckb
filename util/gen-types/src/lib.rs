#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

mod conversion;
pub mod core;
pub mod extension;
mod generated;
pub use generated::packed;
pub mod prelude;
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
