#![cfg_attr(all(test, feature = "dev"), feature(test))]
#[cfg(all(test, feature = "dev"))]
extern crate test;

extern crate bigint;
#[cfg(feature = "bech32")]
#[macro_use]
extern crate crunchy;
#[macro_use]
extern crate failure;
#[macro_use]
extern crate lazy_static;
extern crate faster_hex;
extern crate rand;

#[cfg(feature = "bech32")]
pub mod bech32;
#[cfg(feature = "secp")]
pub mod secp;
