mod syscalls;
mod verify;

use ckb_vm::Error as VMInternalError;

pub use crate::verify::TransactionScriptsVerifier;

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum ScriptError {
    NoScript,
    InvalidReferenceIndex,
    ArgumentError,
    ValidationFailure(u8),
    VMError(VMInternalError),
}
