#[cfg(test)]
extern crate bigint;
extern crate ckb_core as core;
extern crate ckb_vm as vm;
#[cfg(test)]
extern crate crypto;
extern crate fnv;
#[cfg(test)]
extern crate hash;
#[cfg(test)]
extern crate rustc_hex;

mod verify;

pub use verify::TransactionInputVerifier;

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum Error {
    NoScript,
    InvalidReferenceIndex,
    ValidationFailure(u8),
    VMError,
}
