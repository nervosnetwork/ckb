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
extern crate rand;
extern crate rustc_hex;
#[cfg(feature = "rsa")]
extern crate serde;
#[cfg(feature = "rsa")]
#[macro_use]
extern crate serde_derive;

#[cfg(feature = "secp")]
pub mod secp;
#[cfg(feature = "bech32")]
pub mod bech32;
#[cfg(feature = "rsa")]
pub mod rsa;
