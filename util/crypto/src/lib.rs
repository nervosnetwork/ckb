extern crate bigint;
#[macro_use]
extern crate failure;
#[macro_use]
extern crate lazy_static;
extern crate rand;
extern crate rustc_hex;

#[cfg(feature = "secp")]
mod secp;
mod error;
