//! Provides the generated types for CKB

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

mod conversion;
pub mod core;
mod extension;
mod generated;
pub mod prelude;
pub use generated::packed;

//re-exports
pub use molecule::bytes;

cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        #[allow(unused_imports)]
        use std::{vec, borrow};
    } else {
        #[allow(unused_imports)]
        use alloc::{vec, borrow};
    }
}
