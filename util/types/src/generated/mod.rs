//! Generated packed bytes wrappers.

#![allow(warnings)]

#[allow(clippy::all)]
mod blockchain;
#[allow(clippy::all)]
mod extensions;
#[allow(clippy::all)]
mod protocols;

pub mod packed {
    pub use molecule::prelude::{Byte, ByteReader};

    pub use super::blockchain::*;
    pub use super::extensions::*;
    pub use super::protocols::*;
}
