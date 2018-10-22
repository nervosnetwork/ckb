extern crate bigint;
extern crate ckb_core as core;
extern crate ckb_protocol;
extern crate ckb_vm as vm;
#[cfg(test)]
extern crate crypto;
#[cfg(test)]
extern crate faster_hex;
extern crate flatbuffers;
extern crate fnv;
#[cfg(test)]
extern crate hash;
#[macro_use]
extern crate log;
#[cfg(test)]
#[macro_use]
extern crate proptest;

mod syscalls;
mod verify;

use vm::Error as VMInternalError;

pub use verify::TransactionScriptsVerifier;

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum ScriptError {
    NoScript,
    InvalidReferenceIndex,
    ValidationFailure(u8),
    VMError(VMInternalError),
}
