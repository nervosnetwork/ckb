#[cfg(test)]
extern crate bigint;
extern crate ckb_core as core;
extern crate ckb_protocol;
extern crate ckb_vm as vm;
#[cfg(test)]
extern crate crypto;
extern crate flatbuffers;
extern crate fnv;
#[cfg(test)]
extern crate hash;
#[cfg(test)]
extern crate rustc_hex;
#[cfg(test)]
#[macro_use]
extern crate proptest;

mod syscalls;
mod verify;

pub use verify::TransactionInputVerifier;

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum Error {
    NoScript,
    InvalidReferenceIndex,
    ValidationFailure(u8),
    VMError,
}
