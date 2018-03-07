#![allow(unknown_lints)]
#![allow(clippy)]
#![cfg_attr(rustfmt, rustfmt_skip)]

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(asm_available, feature(asm))]

#[cfg(feature = "std")]
extern crate core;
#[macro_use]
extern crate crunchy;
#[macro_use]
extern crate fixed_hash;
#[macro_use]
extern crate uint as uint_crate;

#[cfg(feature = "serialize")]
extern crate bigint_serialize;
#[cfg(feature = "serialize")]
extern crate serde;

mod hash;
mod uint;

pub use fixed_hash::clean_0x;
pub use hash::{H1024, H128, H160, H256, H264, H32, H512, H520, H64, H328};
pub use uint::{U128, U256, U512};

pub type Secret = H256;
pub type Public = H512;
pub type Signature = H520;
