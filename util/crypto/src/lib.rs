#![cfg_attr(all(test, feature = "dev"), feature(test))]
#[cfg(all(test, feature = "dev"))]
extern crate test;

extern crate bigint;
#[macro_use]
extern crate crunchy;
#[macro_use]
extern crate failure;
#[macro_use]
extern crate lazy_static;
extern crate rand;
extern crate rustc_hex;

#[cfg(feature = "secp")]
pub mod secp;
pub mod bech32;
