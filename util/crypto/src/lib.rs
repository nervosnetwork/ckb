#![cfg_attr(all(test, feature = "dev"), feature(test))]
#[cfg(all(test, feature = "dev"))]
extern crate test;

#[cfg(feature = "bech32")]
pub mod bech32;
#[cfg(feature = "secp")]
pub mod secp;
