//! # The Generated Types Library
//!
//! This Library provides the generated types for CKB.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

pub mod base;
mod conversion;
pub mod extension;
mod generated;
pub mod prelude;
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
